use futures_util::StreamExt;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing;

use crate::feishu::bridge::LarkCliBridge;
use crate::feishu::types::FeishuEvent;

/// 运行网关编排循环
///
/// 流程:
/// 1. 监听飞书消息
/// 2. 收到消息 → 回复"已收到"
/// 3. 通过 stream-json 流式调用 Claude，每 3 秒发一次进度
/// 4. 完成后发送完整结果回飞书
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

    // 3. 启动飞书事件监听
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

    // 4. 事件循环
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
        let chat_id = event.chat_id.clone();
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
        tracing::info!(chat_id = %chat_id, "已发送确认消息");

        // 流式调用 Claude
        println!("正在调用 Claude (streaming)...");
        tracing::info!(
            chat_id = %chat_id,
            sender = %sender_id,
            message = %user_message,
            "开始流式调用 Claude"
        );

        if let Err(e) = process_with_claude_stream(feishu_bridge, &chat_id, &user_message).await {
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

    Err("lark-cli 事件流意外结束".to_string())
}

/// 调用 claude --print --output-format stream-json，边流式读取边推送飞书进度
async fn process_with_claude_stream(
    feishu_bridge: &LarkCliBridge,
    chat_id: &str,
    prompt: &str,
) -> Result<(), String> {
    let mut child = Command::new("claude")
        .args([
            "--print",
            "--verbose",
            "--output-format",
            "stream-json",
            "--include-partial-messages",
            prompt,
        ])
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

    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }

        // 解析 NDJSON，只累积文本，不发送中间态消息
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&line) {
            let type_str = json.get("type").and_then(|v| v.as_str());

            if let Some("stream_event") = type_str {
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
            }
        }
    }

    // 等待子进程退出
    let _ = child.wait().await;

    let final_response = full_response.trim().to_string();

    if final_response.is_empty() {
        return Err("Claude 返回为空".to_string());
    }

    // 只发送最终结果，不发送中间态消息
    let _ = crate::feishu::send::send_text(
        feishu_bridge,
        chat_id,
        &format!("💡 处理完成:\n\n{}", final_response),
    )
    .await;

    tracing::info!(
        chat_id = %chat_id,
        total_chars = final_response.len(),
        response = %final_response,
        "Claude 流式处理完成"
    );

    Ok(())
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
