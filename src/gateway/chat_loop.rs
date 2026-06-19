use futures_util::StreamExt;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing;

use crate::feishu::bridge::LarkCliBridge;
use crate::feishu::types::FeishuEvent;
use crate::gateway::session::{SessionKey, SessionManager};

/// 内置网关命令
enum GatewayCommand {
    /// 手动开启新会话
    New,
    /// 列出活跃会话
    List,
    /// 显示帮助
    Help,
    /// 显示当前会话状态
    Status,
}

/// 运行网关编排循环
///
/// 流程:
/// 1. 监听飞书消息
/// 2. 检查是否是内置命令（如 /new），是则本地处理
/// 3. 检查是否有已有会话，有则 --resume，无则新会话
/// 4. 通过 stream-json 流式调用 Claude
/// 5. 完成后发送完整结果回飞书
pub async fn run_chat_loop(feishu_bridge: &LarkCliBridge) -> Result<(), String> {
    // 1. 检查飞书健康状态
    let health = feishu_bridge.health_check().await;
    if !health.lark_cli_found {
        return Err("lark-cli 未安装。请执行: npm install -g @larksuite/cli".to_string());
    }
    if !health.authenticated {
        return Err("飞书未认证。请先执行: haimen feishu auth login".to_string());
    }

    // 2. 检查 claude CLI 是否可用
    if !check_claude_available().await {
        return Err(
            "claude CLI 未安装。请执行: npm install -g @anthropic-ai/claude-code".to_string(),
        );
    }

    // 3. 加载配置
    let config = crate::config::settings::load_settings()
        .ok()
        .flatten()
        .unwrap_or_default();

    // 4. 获取会话配置
    let idle_timeout = config.gateway.session_idle_timeout_mins;
    let max_turns = config.gateway.session_max_turns;
    let work_dir = config.gateway.work_dir.clone().unwrap_or_else(|| {
        std::env::current_dir()
            .ok()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| ".".to_string())
    });

    // 5. 初始化会话管理器
    let mut session_mgr = SessionManager::new(idle_timeout, max_turns);

    // 6. 启动飞书事件监听
    let mut stream = feishu_bridge
        .stream(&[
            "event",
            "consume",
            "im.message.receive_v1",
            "--as",
            "bot",
            "--quiet",
        ])
        .await?;

    println!("🚀 网关已启动，等待飞书消息... (按 Ctrl+C 退出)");
    tracing::info!(
        idle_timeout_mins = idle_timeout,
        max_turns = max_turns,
        work_dir = %work_dir,
        "网关启动"
    );

    // 7. 事件循环
    while let Some(line_result) = stream.next().await {
        let line = match line_result {
            Ok(l) => l,
            Err(e) => {
                eprintln!("读取事件流错误: {}", e);
                continue;
            }
        };

        if line.trim().is_empty() {
            continue;
        }

        // 解析飞书事件
        let event: FeishuEvent = match serde_json::from_str(&line) {
            Ok(e) => e,
            Err(_) => continue,
        };

        // 只处理文本消息
        if event.message_type != "text" {
            continue;
        }

        // 提取用户消息文本
        let user_message = extract_text_content(&event.content);
        let chat_id: SessionKey = event.chat_id.clone();
        let sender_id = event.sender_id.clone();

        println!("收到消息来自 {}: {}", sender_id, user_message);
        tracing::info!(
            sender = %sender_id,
            chat_id = %chat_id,
            message = %user_message,
            "收到飞书消息"
        );

        // 回复"已收到"
        if let Err(e) = crate::feishu::send::send_text(
            feishu_bridge,
            &chat_id,
            "✅ 已收到你的消息，正在处理...",
        )
        .await
        {
            eprintln!("发送确认消息失败: {}", e);
            tracing::warn!(chat_id = %chat_id, error = %e, "发送确认消息失败");
            continue;
        }

        // 检查是否是内置命令
        if let Some(cmd) = parse_command(&user_message) {
            handle_command(&mut session_mgr, feishu_bridge, &chat_id, cmd).await;
            continue;
        }

        // 获取会话状态
        let (need_new_session, existing_session_id) = session_mgr.get_or_create(&chat_id);

        // 调用 Claude 处理
        println!("正在调用 Claude (streaming)...");
        tracing::info!(
            chat_id = %chat_id,
            need_new_session = need_new_session,
            "开始调用 Claude"
        );

        let result = if need_new_session {
            process_with_claude_stream(&user_message, None).await
        } else {
            process_with_claude_stream(&user_message, existing_session_id.as_deref()).await
        };

        match result {
            Ok((response, new_session_id)) => {
                if need_new_session {
                    session_mgr.create_session(&chat_id, &new_session_id, &work_dir);
                }
                session_mgr.record_turn(&chat_id);

                println!("Claude 处理完成，{} 字符", response.len());
                tracing::info!(
                    chat_id = %chat_id,
                    total_chars = response.len(),
                    session_id = %new_session_id,
                    "Claude 处理完成"
                );

                let _ = crate::feishu::send::send_text(
                    feishu_bridge,
                    &chat_id,
                    &format!("💡 处理完成:\n\n{}", response),
                )
                .await;
            }
            Err(e) => {
                // Resume 失败时自动降级为新会话重试
                if !need_new_session {
                    tracing::warn!(
                        chat_id = %chat_id,
                        error = %e,
                        "Resume 失败，降级为新会话重试"
                    );
                    eprintln!("Resume 失败 ({}), 降级为新会话重试...", e);
                    session_mgr.remove_session(&chat_id);

                    match process_with_claude_stream(&user_message, None).await {
                        Ok((response, new_session_id)) => {
                            session_mgr.create_session(&chat_id, &new_session_id, &work_dir);
                            tracing::info!(
                                chat_id = %chat_id,
                                total_chars = response.len(),
                                "降级重试成功"
                            );
                            let _ = crate::feishu::send::send_text(
                                feishu_bridge,
                                &chat_id,
                                &format!("💡 处理完成:\n\n{}", response),
                            )
                            .await;
                        }
                        Err(e2) => {
                            tracing::error!(
                                chat_id = %chat_id,
                                error = %e2,
                                "降级重试也失败"
                            );
                            let _ = crate::feishu::send::send_text(
                                feishu_bridge,
                                &chat_id,
                                &format!("❌ 处理失败: {}", e2),
                            )
                            .await;
                        }
                    }
                } else {
                    eprintln!("Claude 处理失败: {}", e);
                    tracing::error!(chat_id = %chat_id, error = %e, "Claude 处理失败");
                    let _ = crate::feishu::send::send_text(
                        feishu_bridge,
                        &chat_id,
                        &format!("❌ 处理失败: {}", e),
                    )
                    .await;
                }
            }
        }
    }

    Err("lark-cli 事件流意外结束".to_string())
}

/// 调用 claude --print 流式处理
///
/// - `session_id`: `None` 表示新会话，`Some(id)` 表示继续已有会话
/// - 返回: `(完整回复文本, 新的 session_id)`
async fn process_with_claude_stream(
    prompt: &str,
    session_id: Option<&str>,
) -> Result<(String, String), String> {
    let mut args: Vec<String> = vec![
        "--print".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
        "--include-partial-messages".to_string(),
    ];

    // 如果传入了 session_id，加上 --resume
    if let Some(sid) = session_id {
        args.push("--resume".to_string());
        args.push(sid.to_string());
    }

    args.push(prompt.to_string());

    let mut child = Command::new("claude")
        .args(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("启动 claude 失败: {}", e))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "无法获取 claude stdout".to_string())?;

    let reader = BufReader::new(stdout);
    let mut lines = reader.lines();

    let mut full_response = String::new();
    let mut extracted_session_id: Option<String> = None;
    let mut system_init_parsed = false;

    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }

        let json: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let type_str = json.get("type").and_then(|v| v.as_str());

        match type_str {
            // 从 system/init 消息提取 session_id
            Some("system") if !system_init_parsed => {
                system_init_parsed = true;
                if let Some(sid) = json.get("session_id").and_then(|v| v.as_str()) {
                    extracted_session_id = Some(sid.to_string());
                    tracing::debug!(session_id = %sid, "提取到 session_id");
                }
            }

            // 累积流式文本
            Some("stream_event") => {
                // 格式1: {"type":"stream_event","event":{"type":"content_block_delta",...}}
                if let Some(event) = json.get("event") {
                    if let Some("content_block_delta") = event.get("type").and_then(|v| v.as_str())
                    {
                        if let Some(delta) = event.get("delta") {
                            if let Some("text_delta") = delta.get("type").and_then(|v| v.as_str()) {
                                if let Some(text) = delta.get("text").and_then(|v| v.as_str()) {
                                    full_response.push_str(text);
                                }
                            }
                        }
                    }
                }
                // 格式2: {"type":"stream_event","event_type":"content_block_delta","delta":{...}}
                if full_response.is_empty() {
                    if let Some("content_block_delta") =
                        json.get("event_type").and_then(|v| v.as_str())
                    {
                        if let Some(delta) = json.get("delta") {
                            if let Some("text_delta") = delta.get("type").and_then(|v| v.as_str()) {
                                if let Some(text) = delta.get("text").and_then(|v| v.as_str()) {
                                    full_response.push_str(text);
                                }
                            }
                        }
                    }
                }
            }

            // 备选：从 assistant 消息中提取完整文本（非流式模式兜底）
            Some("assistant") if full_response.is_empty() => {
                if let Some(content) = json.get("content").and_then(|c| c.as_array()) {
                    for block in content {
                        if let Some("text") = block.get("type").and_then(|v| v.as_str()) {
                            if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                                full_response.push_str(text);
                            }
                        }
                    }
                }
            }

            // 从 result 消息兜底提取 session_id
            Some("result") if extracted_session_id.is_none() => {
                if let Some(sid) = json.get("session_id").and_then(|v| v.as_str()) {
                    extracted_session_id = Some(sid.to_string());
                }
            }

            _ => {}
        }
    }

    // 等待子进程退出
    let _ = child.wait().await;

    let final_response = full_response.trim().to_string();
    let final_session_id =
        extracted_session_id.ok_or_else(|| "无法从 claude 输出中提取 session_id".to_string())?;

    if final_response.is_empty() {
        return Err("Claude 返回为空".to_string());
    }

    Ok((final_response, final_session_id))
}

/// 检查 claude CLI 是否可用
async fn check_claude_available() -> bool {
    Command::new("claude")
        .arg("--version")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 从飞书文本消息内容中提取文本
fn extract_text_content(content: &str) -> String {
    if let Ok(obj) = serde_json::from_str::<serde_json::Value>(content) {
        if let Some(text) = obj.get("text").and_then(|t| t.as_str()) {
            return text.to_string();
        }
    }
    content
        .trim_start_matches('"')
        .trim_end_matches('"')
        .to_string()
}

/// 从消息中解析内置命令
fn parse_command(msg: &str) -> Option<GatewayCommand> {
    let trimmed = msg.trim();
    match trimmed {
        "/new" | "/新会话" => Some(GatewayCommand::New),
        "/list" | "/会话列表" => Some(GatewayCommand::List),
        "/help" | "/帮助" => Some(GatewayCommand::Help),
        "/status" | "/状态" => Some(GatewayCommand::Status),
        _ => None,
    }
}

/// 处理内置命令
async fn handle_command(
    session_mgr: &mut SessionManager,
    bridge: &LarkCliBridge,
    chat_id: &str,
    cmd: GatewayCommand,
) {
    match cmd {
        GatewayCommand::New => {
            session_mgr.remove_session(&chat_id.to_string());
            tracing::info!(chat_id = %chat_id, "用户手动开启新会话");
            let _ = crate::feishu::send::send_text(
                bridge,
                chat_id,
                "✅ 已创建新会话。发消息给我就开始吧！",
            )
            .await;
        }
        GatewayCommand::List => {
            let sessions = session_mgr.list_sessions();
            let msg = if sessions.is_empty() {
                "当前没有活跃会话。发消息给我会自动创建。".to_string()
            } else {
                let mut lines = vec!["📋 当前活跃会话:".to_string()];
                for (key, info) in sessions {
                    lines.push(format!(
                        "  • {} ({} 轮, 最后活跃: {})",
                        key,
                        info.turn_count,
                        info.last_active.format("%H:%M:%S"),
                    ));
                }
                lines.join("\n")
            };
            let _ = crate::feishu::send::send_text(bridge, chat_id, &msg).await;
        }
        GatewayCommand::Help => {
            let help = [
                "🤖 haimen 网关命令指南:",
                "",
                "  /new 或 /新会话  开启新会话",
                "  /list 或 /会话列表  查看活跃会话",
                "  /status 或 /状态  查看当前会话状态",
                "  /help 或 /帮助  显示此帮助",
                "",
                "其他消息会自动发送给 Claude Code 处理，",
                "同一对话的上下文会自动保持。",
            ]
            .join("\n");
            let _ = crate::feishu::send::send_text(bridge, chat_id, &help).await;
        }
        GatewayCommand::Status => {
            let status = match session_mgr.get_session(&chat_id.to_string()) {
                Some(info) => {
                    format!(
                        "📊 当前会话状态:\n  轮次: {}/{}\n  创建: {}\n  最后活跃: {}\n  工作目录: {}",
                        info.turn_count,
                        info.max_turns,
                        info.created_at.format("%H:%M:%S"),
                        info.last_active.format("%H:%M:%S"),
                        info.cwd,
                    )
                }
                None => "当前没有活跃会话。发消息给我会自动创建。".to_string(),
            };
            let _ = crate::feishu::send::send_text(bridge, chat_id, &status).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_command_new() {
        assert!(matches!(parse_command("/new"), Some(GatewayCommand::New)));
        assert!(matches!(
            parse_command("/新会话"),
            Some(GatewayCommand::New)
        ));
    }

    #[test]
    fn test_parse_command_list() {
        assert!(matches!(parse_command("/list"), Some(GatewayCommand::List)));
        assert!(matches!(
            parse_command("/会话列表"),
            Some(GatewayCommand::List)
        ));
    }

    #[test]
    fn test_parse_command_help() {
        assert!(matches!(parse_command("/help"), Some(GatewayCommand::Help)));
        assert!(matches!(parse_command("/帮助"), Some(GatewayCommand::Help)));
    }

    #[test]
    fn test_parse_command_status() {
        assert!(matches!(
            parse_command("/status"),
            Some(GatewayCommand::Status)
        ));
        assert!(matches!(
            parse_command("/状态"),
            Some(GatewayCommand::Status)
        ));
    }

    #[test]
    fn test_parse_command_not_a_command() {
        assert!(parse_command("你好").is_none());
        assert!(parse_command("帮我改代码").is_none());
        assert!(parse_command("").is_none());
        assert!(
            parse_command("  /new").is_some(),
            "trimmed /new should work"
        );
    }

    #[test]
    fn test_parse_command_none_for_normal_text() {
        assert!(parse_command("帮我看看这个bug").is_none());
        assert!(parse_command("分析项目结构").is_none());
    }

    #[test]
    fn test_extract_text_content_json() {
        let content = r#"{"text":"你好世界"}"#;
        assert_eq!(extract_text_content(content), "你好世界");
    }

    #[test]
    fn test_extract_text_content_plain() {
        assert_eq!(extract_text_content("\"hello\""), "hello");
        assert_eq!(extract_text_content("plain text"), "plain text");
    }

    #[test]
    fn test_extract_text_content_empty() {
        assert_eq!(extract_text_content(""), "");
    }
}
