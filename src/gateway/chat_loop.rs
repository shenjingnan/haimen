use futures_util::StreamExt;
use tracing;

use crate::config::settings::GatewayConfig;
use crate::gateway::channel::MessageChannel;
use crate::gateway::provider::AgentProvider;
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

/// 运行通用网关编排循环
///
/// 流程:
/// 1. 健康检查
/// 2. 从 channel.listen() 获取 Message 流
/// 3. 检查内置命令（如 /new），是则本地处理
/// 4. 会话管理（get_or_create / resume）
/// 5. 调用 agent.process() 处理消息
/// 6. 通过 channel.send() 发送回复
///
/// 不依赖任何具体 Channel 或 Agent 类型。
pub async fn run_chat_loop<C, A>(
    channel: &C,
    agent: &A,
    config: &GatewayConfig,
) -> Result<(), String>
where
    C: MessageChannel + ?Sized,
    A: AgentProvider + ?Sized,
{
    // 1. 健康检查
    channel.health_check().await?;
    agent.check_available().await?;

    // 2. 加载会话配置
    let idle_timeout = config.session_idle_timeout_mins;
    let max_turns = config.session_max_turns;
    let work_dir = config.work_dir.clone().unwrap_or_else(|| {
        std::env::current_dir()
            .ok()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| ".".to_string())
    });
    let mut session_mgr = SessionManager::new(idle_timeout, max_turns);

    // 3. 启动消息流
    let mut stream = channel.listen().await?;

    tracing::info!(
        channel = %channel.name(),
        agent = %agent.name(),
        idle_timeout_mins = idle_timeout,
        max_turns = max_turns,
        "网关已启动"
    );

    // 4. 事件循环
    while let Some(message) = stream.next().await {
        let chat_id: SessionKey = message.conversation_id.clone();

        // 发送确认
        if let Err(e) = channel
            .send(&chat_id, "✅ 已收到你的消息，正在处理...")
            .await
        {
            tracing::warn!(chat_id = %chat_id, error = %e, "发送确认消息失败");
            continue;
        }

        tracing::info!(
            sender = %message.sender_id,
            chat_id = %chat_id,
            message = %message.content,
            "收到消息"
        );

        // 检查内置命令
        if let Some(cmd) = parse_command(&message.content) {
            handle_command(&mut session_mgr, channel, &chat_id, cmd).await;
            continue;
        }

        // 会话管理
        let (need_new_session, existing_session_id) = session_mgr.get_or_create(&chat_id);

        // 调用 Agent 处理
        let result = if need_new_session {
            agent.process(&message.content, None).await
        } else {
            agent
                .process(&message.content, existing_session_id.as_deref())
                .await
        };

        match result {
            Ok((response, new_session_id)) => {
                if need_new_session {
                    session_mgr.create_session(&chat_id, &new_session_id, &work_dir);
                }
                session_mgr.record_turn(&chat_id);

                tracing::info!(
                    chat_id = %chat_id,
                    total_chars = response.len(),
                    session_id = %new_session_id,
                    "Agent 处理完成"
                );

                let _ = channel
                    .send(&chat_id, &format!("💡 处理完成:\n\n{}", response))
                    .await;
            }
            Err(e) => {
                // Resume 失败时自动降级为新会话重试
                if !need_new_session {
                    tracing::warn!(chat_id = %chat_id, error = %e, "Resume 失败，降级为新会话重试");
                    session_mgr.remove_session(&chat_id);

                    match agent.process(&message.content, None).await {
                        Ok((response, new_session_id)) => {
                            session_mgr.create_session(&chat_id, &new_session_id, &work_dir);
                            tracing::info!(chat_id = %chat_id, "降级重试成功");
                            let _ = channel
                                .send(&chat_id, &format!("💡 处理完成:\n\n{}", response))
                                .await;
                        }
                        Err(e2) => {
                            tracing::error!(chat_id = %chat_id, error = %e2, "降级重试也失败");
                            let _ = channel
                                .send(&chat_id, &format!("❌ 处理失败: {}", e2))
                                .await;
                        }
                    }
                } else {
                    tracing::error!(chat_id = %chat_id, error = %e, "Agent 处理失败");
                    let _ = channel.send(&chat_id, &format!("❌ 处理失败: {}", e)).await;
                }
            }
        }
    }

    Err("消息流意外结束".to_string())
}

/// 运行多连接器统一网关编排循环
///
/// 合并多个连接器的消息流，统一调度 Agent 处理。
/// 每条消息按 connector_name 路由回正确的连接器回复。
/// Session key 加 connector_name 前缀，防止跨连接器 conversation_id 碰撞。
pub async fn run_unified_gateway(
    channels: Vec<(String, Box<dyn MessageChannel>)>,
    agent: &dyn AgentProvider,
    config: &GatewayConfig,
) -> Result<(), String> {
    if channels.is_empty() {
        tracing::warn!("没有可用的连接器");
        return Ok(());
    }

    // 1. 加载会话配置
    let idle_timeout = config.session_idle_timeout_mins;
    let max_turns = config.session_max_turns;
    let work_dir = config.work_dir.clone().unwrap_or_else(|| {
        std::env::current_dir()
            .ok()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| ".".to_string())
    });
    let mut session_mgr = SessionManager::new(idle_timeout, max_turns);

    // 2. 启动所有连接器的消息流，标记 connector_name
    let mut tagged_streams = Vec::new();
    for (name, channel) in &channels {
        let stream = channel
            .listen()
            .await
            .map_err(|e| format!("连接器 '{}' listen 失败: {}", name, e))?;
        let cn = name.clone();
        let tagged = stream.map(move |msg| (cn.clone(), msg));
        tagged_streams.push(tagged);
    }

    let channel_names: Vec<&str> = channels.iter().map(|(n, _)| n.as_str()).collect();
    tracing::info!(
        channels = ?channel_names,
        agent = %agent.name(),
        idle_timeout_mins = idle_timeout,
        max_turns = max_turns,
        "多连接器网关已启动"
    );

    // 3. 合并流
    let mut merged = futures_util::stream::select_all(tagged_streams);

    // 4. 事件循环
    while let Some((connector_name, message)) = merged.next().await {
        // 用 connector_name:conversation_id 作为 session key，防止跨连接器碰撞
        let chat_id: SessionKey = format!("{}:{}", connector_name, message.conversation_id);

        // 找到对应的连接器用于回复
        let channel: &dyn MessageChannel = match channels.iter().find(|(n, _)| n == &connector_name)
        {
            Some((_, ch)) => &**ch,
            None => {
                tracing::warn!(connector = %connector_name, "找不到对应的连接器");
                continue;
            }
        };

        // 发送确认
        if let Err(e) = channel
            .send(&message.conversation_id, "✅ 已收到你的消息，正在处理...")
            .await
        {
            tracing::warn!(chat_id = %chat_id, error = %e, "发送确认消息失败");
            continue;
        }

        tracing::info!(
            connector = %connector_name,
            sender = %message.sender_id,
            chat_id = %chat_id,
            message = %message.content,
            "收到消息"
        );

        // 检查内置命令
        if let Some(cmd) = parse_command(&message.content) {
            handle_command_for_channel(&mut session_mgr, channel, &chat_id, cmd).await;
            continue;
        }

        // 会话管理
        let (need_new_session, existing_session_id) = session_mgr.get_or_create(&chat_id);

        // 调用 Agent 处理
        let result = if need_new_session {
            agent.process(&message.content, None).await
        } else {
            agent
                .process(&message.content, existing_session_id.as_deref())
                .await
        };

        match result {
            Ok((response, new_session_id)) => {
                if need_new_session {
                    session_mgr.create_session(&chat_id, &new_session_id, &work_dir);
                }
                session_mgr.record_turn(&chat_id);

                tracing::info!(
                    connector = %connector_name,
                    chat_id = %chat_id,
                    total_chars = response.len(),
                    session_id = %new_session_id,
                    "Agent 处理完成"
                );

                let _ = channel
                    .send(
                        &message.conversation_id,
                        &format!("💡 处理完成:\n\n{}", response),
                    )
                    .await;
            }
            Err(e) => {
                if !need_new_session {
                    tracing::warn!(chat_id = %chat_id, error = %e, "Resume 失败，降级为新会话重试");
                    session_mgr.remove_session(&chat_id);

                    match agent.process(&message.content, None).await {
                        Ok((response, new_session_id)) => {
                            session_mgr.create_session(&chat_id, &new_session_id, &work_dir);
                            tracing::info!(chat_id = %chat_id, "降级重试成功");
                            let _ = channel
                                .send(
                                    &message.conversation_id,
                                    &format!("💡 处理完成:\n\n{}", response),
                                )
                                .await;
                        }
                        Err(e2) => {
                            tracing::error!(chat_id = %chat_id, error = %e2, "降级重试也失败");
                            let _ = channel
                                .send(&message.conversation_id, &format!("❌ 处理失败: {}", e2))
                                .await;
                        }
                    }
                } else {
                    tracing::error!(chat_id = %chat_id, error = %e, "Agent 处理失败");
                    let _ = channel
                        .send(&message.conversation_id, &format!("❌ 处理失败: {}", e))
                        .await;
                }
            }
        }
    }

    Err("所有消息流意外结束".to_string())
}

/// 运行 Echo 循环（Channel 调试模式）
///
/// 只启动 Channel 监听，收到消息后直接 echo 回去，不经过 Agent 处理。
/// 用于验证通道连通性和消息格式。
pub async fn run_echo_loop<C: MessageChannel + ?Sized>(channel: &C) -> Result<(), String> {
    channel.health_check().await?;

    let mut stream = channel.listen().await?;

    tracing::info!(channel = %channel.name(), "Echo 模式已启动");

    while let Some(message) = stream.next().await {
        tracing::info!(
            sender = %message.sender_id,
            conversation_id = %message.conversation_id,
            "收到消息 (echo)"
        );

        let echo = format!("🔄 Echo: {}", message.content);
        if let Err(e) = channel.send(&message.conversation_id, &echo).await {
            tracing::warn!(error = %e, "发送 echo 失败");
        }
    }

    Err("消息流意外结束".to_string())
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

/// 处理内置命令（单连接器版）
async fn handle_command<C: MessageChannel + ?Sized>(
    session_mgr: &mut SessionManager,
    channel: &C,
    chat_id: &str,
    cmd: GatewayCommand,
) {
    match cmd {
        GatewayCommand::New => {
            session_mgr.remove_session(&chat_id.to_string());
            tracing::info!(chat_id = %chat_id, "用户手动开启新会话");
            let _ = channel
                .send(chat_id, "✅ 已创建新会话。发消息给我就开始吧！")
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
            let _ = channel.send(chat_id, &msg).await;
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
                "其他消息会自动发送给 AI 处理，",
                "同一对话的上下文会自动保持。",
            ]
            .join("\n");
            let _ = channel.send(chat_id, &help).await;
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
            let _ = channel.send(chat_id, &status).await;
        }
    }
}

/// 处理内置命令（多连接器版，使用 trait object）
async fn handle_command_for_channel(
    session_mgr: &mut SessionManager,
    channel: &dyn MessageChannel,
    chat_id: &str,
    cmd: GatewayCommand,
) {
    match cmd {
        GatewayCommand::New => {
            session_mgr.remove_session(&chat_id.to_string());
            tracing::info!(chat_id = %chat_id, "用户手动开启新会话");
            let _ = channel
                .send(chat_id, "✅ 已创建新会话。发消息给我就开始吧！")
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
            let _ = channel.send(chat_id, &msg).await;
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
                "其他消息会自动发送给 AI 处理，",
                "同一对话的上下文会自动保持。",
            ]
            .join("\n");
            let _ = channel.send(chat_id, &help).await;
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
            let _ = channel.send(chat_id, &status).await;
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
}
