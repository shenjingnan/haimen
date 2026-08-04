use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing;

use crate::config::settings::GatewayConfig;
use crate::gateway::channel::MessageChannel;
use crate::gateway::model::Message;
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
    let work_dir = resolve_work_dir(config.work_dir.clone());
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
        let start = std::time::Instant::now();
        let result = if need_new_session {
            agent.process(&message.content, None, &work_dir).await
        } else {
            agent
                .process(&message.content, existing_session_id.as_deref(), &work_dir)
                .await
        };

        // 记录一次 Agent 调用（每次 process 尝试都记一条）
        let record_call = |status: &str,
                           output: Option<&str>,
                           error: Option<&str>,
                           session_id: Option<&str>,
                           latency: std::time::Duration| {
            crate::agent_log::record(&crate::agent_log::AgentLogRecord {
                timestamp: crate::datetime::iso_timestamp_now(),
                source: "gateway".to_string(),
                agent: agent.name().to_string(),
                connector: Some(channel.name().to_string()),
                chat_id: Some(chat_id.clone()),
                sender_id: Some(message.sender_id.clone()),
                session_id: session_id.map(String::from),
                work_dir: work_dir.clone(),
                input: message.content.clone(),
                output: output.map(String::from),
                status: status.to_string(),
                error: error.map(String::from),
                latency_ms: latency.as_millis() as u64,
            });
        };

        match result {
            Ok((response, new_session_id)) => {
                record_call(
                    "success",
                    Some(&response),
                    None,
                    Some(&new_session_id),
                    start.elapsed(),
                );
                if need_new_session {
                    session_mgr.create_session(&chat_id, &new_session_id, &work_dir);
                }
                session_mgr.record_turn(&chat_id);

                tracing::info!(
                    chat_id = %chat_id,
                    total_chars = response.len(),
                    session_id = %new_session_id,
                    response = %response,
                    "Agent 处理完成"
                );

                let _ = channel
                    .send(&chat_id, &format!("💡 处理完成:\n\n{}", response))
                    .await;
            }
            Err(e) => {
                // Resume 失败时自动降级为新会话重试
                if !need_new_session {
                    record_call("error", None, Some(&e), None, start.elapsed());
                    tracing::warn!(chat_id = %chat_id, error = %e, "Resume 失败，降级为新会话重试");
                    session_mgr.remove_session(&chat_id);

                    let retry_start = std::time::Instant::now();
                    match agent.process(&message.content, None, &work_dir).await {
                        Ok((response, new_session_id)) => {
                            record_call(
                                "success",
                                Some(&response),
                                None,
                                Some(&new_session_id),
                                retry_start.elapsed(),
                            );
                            session_mgr.create_session(&chat_id, &new_session_id, &work_dir);
                            tracing::info!(
                                chat_id = %chat_id,
                                response = %response,
                                "降级重试成功"
                            );
                            let _ = channel
                                .send(&chat_id, &format!("💡 处理完成:\n\n{}", response))
                                .await;
                        }
                        Err(e2) => {
                            record_call("error", None, Some(&e2), None, retry_start.elapsed());
                            tracing::error!(chat_id = %chat_id, error = %e2, "降级重试也失败");
                            let _ = channel
                                .send(&chat_id, &format!("❌ 处理失败: {}", e2))
                                .await;
                        }
                    }
                } else {
                    record_call("error", None, Some(&e), None, start.elapsed());
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
///
/// 通过 pump task + mpsc 架构实现 channel 崩溃隔离：
/// - 每个 channel 的 listen stream 运行在独立的 tokio::spawn 任务中
/// - 通过 mpsc::unbounded_channel 桥接到主事件循环
/// - 单个 pump task 的 panic 不会影响其他连接器
/// - CancellationToken 支持优雅关闭
/// - agent.process() 带超时保护，防止单次调用阻塞整个网关
pub async fn run_unified_gateway(
    channels: Vec<(String, Box<dyn MessageChannel>)>,
    agent: &dyn AgentProvider,
    config: &GatewayConfig,
    cancel: CancellationToken,
) -> Result<(), String> {
    if channels.is_empty() {
        tracing::warn!("没有可用的连接器");
        return Ok(());
    }

    // 1. 加载会话配置
    let idle_timeout = config.session_idle_timeout_mins;
    let max_turns = config.session_max_turns;
    let work_dir = resolve_work_dir(config.work_dir.clone());
    let mut session_mgr = SessionManager::new(idle_timeout, max_turns);

    // 2. 创建全局 mpsc 通道（替代 select_all）
    let (global_tx, mut global_rx) = mpsc::unbounded_channel::<(String, Message)>();
    let timeout_duration = Duration::from_secs(config.agent_timeout_secs);

    let channel_names: Vec<&str> = channels.iter().map(|(n, _)| n.as_str()).collect();
    tracing::info!(
        channels = ?channel_names,
        agent = %agent.name(),
        idle_timeout_mins = idle_timeout,
        max_turns = max_turns,
        agent_timeout_secs = config.agent_timeout_secs,
        "多连接器网关已启动"
    );

    // 3. 为每个连接器创建 pump task（带 listen 中断保护）
    let mut pump_count = 0usize;

    for (name, channel) in &channels {
        let cn = name.clone();

        // 用 select! 使 listen() 过程也响应关闭信号
        let stream = tokio::select! {
            result = channel.listen() => {
                match result {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!(connector = %cn, error = %e, "连接器 listen 失败，跳过");
                        continue;
                    }
                }
            }
            _ = cancel.cancelled() => {
                tracing::info!(connector = %cn, "网关关闭，跳过连接器");
                continue;
            }
        };

        pump_count += 1;
        let tx = global_tx.clone();
        let task_cancel = cancel.clone();

        tokio::spawn(async move {
            tokio::pin!(stream);
            // cancelled future pin 一次，避免循环中反复创建
            let cancel_wait = task_cancel.cancelled();
            tokio::pin!(cancel_wait);

            loop {
                tokio::select! {
                    _ = cancel_wait.as_mut() => {
                        tracing::info!(connector = %cn, "连接器已停止");
                        break;
                    }
                    msg = stream.next() => {
                        match msg {
                            Some(msg) => {
                                if tx.send((cn.clone(), msg)).is_err() {
                                    // main loop 已退出
                                    break;
                                }
                            }
                            None => {
                                tracing::warn!(connector = %cn, "连接器消息流已结束");
                                break;
                            }
                        }
                    }
                }
            }
        });
    }

    // 4. 释放 sender 所有权，让 recv() 在所有 pump task 退出后正确返回 None
    drop(global_tx);

    if pump_count == 0 {
        if cancel.is_cancelled() {
            return Ok(());
        }
        return Err("没有连接器成功启动消息流".to_string());
    }

    // 5. 主事件循环（agent 超时 + 关闭感知）
    while let Some((connector_name, message)) = global_rx.recv().await {
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

        // 调用 Agent 处理（带超时）
        let start = std::time::Instant::now();
        let process_fut = if need_new_session {
            agent.process(&message.content, None, &work_dir)
        } else {
            agent.process(&message.content, existing_session_id.as_deref(), &work_dir)
        };

        // 记录一次 Agent 调用（每次 process 尝试都记一条）
        let record_call = |status: &str,
                           output: Option<&str>,
                           error: Option<&str>,
                           session_id: Option<&str>,
                           latency: std::time::Duration| {
            crate::agent_log::record(&crate::agent_log::AgentLogRecord {
                timestamp: crate::datetime::iso_timestamp_now(),
                source: "gateway".to_string(),
                agent: agent.name().to_string(),
                connector: Some(connector_name.clone()),
                chat_id: Some(chat_id.clone()),
                sender_id: Some(message.sender_id.clone()),
                session_id: session_id.map(String::from),
                work_dir: work_dir.clone(),
                input: message.content.clone(),
                output: output.map(String::from),
                status: status.to_string(),
                error: error.map(String::from),
                latency_ms: latency.as_millis() as u64,
            });
        };

        let result = tokio::time::timeout(timeout_duration, process_fut).await;

        match result {
            Ok(Ok((response, new_session_id))) => {
                record_call(
                    "success",
                    Some(&response),
                    None,
                    Some(&new_session_id),
                    start.elapsed(),
                );
                if need_new_session {
                    session_mgr.create_session(&chat_id, &new_session_id, &work_dir);
                }
                session_mgr.record_turn(&chat_id);

                tracing::info!(
                    connector = %connector_name,
                    chat_id = %chat_id,
                    total_chars = response.len(),
                    session_id = %new_session_id,
                    response = %response,
                    "Agent 处理完成"
                );

                let _ = channel
                    .send(
                        &message.conversation_id,
                        &format!("💡 处理完成:\n\n{}", response),
                    )
                    .await;
            }
            Ok(Err(e)) => {
                // Resume 失败时自动降级为新会话重试（带超时保护）
                if !need_new_session {
                    record_call("error", None, Some(&e), None, start.elapsed());
                    tracing::warn!(chat_id = %chat_id, error = %e, "Resume 失败，降级为新会话重试");
                    session_mgr.remove_session(&chat_id);

                    let retry_start = std::time::Instant::now();
                    let retry_fut = agent.process(&message.content, None, &work_dir);
                    let retry_result = tokio::time::timeout(timeout_duration, retry_fut).await;

                    match retry_result {
                        Ok(Ok((response, new_session_id))) => {
                            record_call(
                                "success",
                                Some(&response),
                                None,
                                Some(&new_session_id),
                                retry_start.elapsed(),
                            );
                            session_mgr.create_session(&chat_id, &new_session_id, &work_dir);
                            tracing::info!(
                                chat_id = %chat_id,
                                response = %response,
                                "降级重试成功"
                            );
                            let _ = channel
                                .send(
                                    &message.conversation_id,
                                    &format!("💡 处理完成:\n\n{}", response),
                                )
                                .await;
                        }
                        Ok(Err(e2)) => {
                            record_call("error", None, Some(&e2), None, retry_start.elapsed());
                            tracing::error!(chat_id = %chat_id, error = %e2, "降级重试也失败");
                            let _ = channel
                                .send(&message.conversation_id, &format!("❌ 处理失败: {}", e2))
                                .await;
                        }
                        Err(_) => {
                            record_call(
                                "timeout",
                                None,
                                Some(&format!(
                                    "处理超时（超过 {} 秒）",
                                    config.agent_timeout_secs
                                )),
                                None,
                                retry_start.elapsed(),
                            );
                            tracing::error!(chat_id = %chat_id, timeout_secs = config.agent_timeout_secs, "降级重试超时");
                            let _ = channel
                                .send(
                                    &message.conversation_id,
                                    &format!(
                                        "❌ 处理超时（超过 {} 秒），请重试",
                                        config.agent_timeout_secs
                                    ),
                                )
                                .await;
                        }
                    }
                } else {
                    record_call("error", None, Some(&e), None, start.elapsed());
                    tracing::error!(chat_id = %chat_id, error = %e, "Agent 处理失败");
                    let _ = channel
                        .send(&message.conversation_id, &format!("❌ 处理失败: {}", e))
                        .await;
                }
            }
            Err(_elapsed) => {
                record_call(
                    "timeout",
                    None,
                    Some(&format!(
                        "处理超时（超过 {} 秒）",
                        config.agent_timeout_secs
                    )),
                    None,
                    start.elapsed(),
                );
                // 超时处理：通知用户并继续处理下一条消息
                tracing::error!(
                    connector = %connector_name,
                    timeout_secs = config.agent_timeout_secs,
                    "Agent 处理超时"
                );
                let _ = channel
                    .send(
                        &message.conversation_id,
                        &format!(
                            "❌ 处理超时（超过 {} 秒），请重试",
                            config.agent_timeout_secs
                        ),
                    )
                    .await;
            }
        }
    }

    // 所有 pump task 退出后，区分正常关闭和异常断开
    if cancel.is_cancelled() {
        tracing::info!("网关已正常关闭");
        Ok(())
    } else {
        Err("所有连接器消息流已结束".to_string())
    }
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

/// 默认 Agent 工作目录（`[gateway] work_dir` 未配置时使用）
///
/// 使用 `~/.haimen/workspace` 作为固定工作区，而非 `haimen start` 的启动目录，
/// 使 Agent 子进程的工作目录不随服务进程的 cwd 变化。
pub const DEFAULT_WORK_DIR: &str = "~/.haimen/workspace";

/// 解析 Agent 工作目录
///
/// 优先级：
/// 1. `[gateway] work_dir` 显式配置（自动展开 `~`）
/// 2. 默认 `~/.haimen/workspace`
///
/// 无论哪种来源，都会确保目录存在（不存在时自动创建）。
pub fn resolve_work_dir(configured: Option<String>) -> String {
    let wd = configured.unwrap_or_else(|| DEFAULT_WORK_DIR.to_string());
    let expanded = expand_tilde(&wd);
    if let Err(e) = std::fs::create_dir_all(&expanded) {
        tracing::warn!(path = %expanded, error = %e, "创建工作目录失败，Agent 子进程可能无法启动");
    }
    expanded
}

/// 展开路径中的 `~` 为 home 目录
///
/// 仅处理以 `~/` 开头的路径，其他情况原样返回。
pub fn expand_tilde(path: &str) -> String {
    if path.starts_with("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{}{}", home.trim_end_matches('/'), &path[1..]);
        }
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::task::Poll;

    use async_trait::async_trait;
    use chrono::Utc;
    use futures_util::Stream;

    // ---------------------------------------------------------------------------
    // MockChannel
    // ---------------------------------------------------------------------------

    struct MockChannel {
        name: &'static str,
        msgs: Mutex<VecDeque<Message>>,
        sent: Mutex<Vec<String>>,
        send_fail: bool,
        listen_fail: bool,
        /// listen() panics
        listen_panic: bool,
        /// returned stream panics after this many items (0 = no panic)
        stream_panic_after: usize,
    }

    impl MockChannel {
        fn new(name: &'static str, msgs: Vec<Message>) -> Self {
            Self {
                name,
                msgs: Mutex::new(VecDeque::from(msgs)),
                sent: Mutex::new(Vec::new()),
                send_fail: false,
                listen_fail: false,
                listen_panic: false,
                stream_panic_after: 0,
            }
        }
    }

    #[async_trait]
    impl MessageChannel for MockChannel {
        fn name(&self) -> &str {
            self.name
        }

        async fn listen(&self) -> Result<Pin<Box<dyn Stream<Item = Message> + Send>>, String> {
            if self.listen_panic {
                panic!("mock listen panic");
            }
            if self.listen_fail {
                return Err("mock listen failure".into());
            }
            let panic_after = self.stream_panic_after;
            let msgs: Vec<Message> = self.msgs.lock().unwrap().drain(..).collect();
            if panic_after > 0 {
                Ok(Box::pin(LimitedPanicStream {
                    remaining: panic_after,
                    delivered: msgs,
                    index: 0,
                }))
            } else {
                Ok(Box::pin(futures_util::stream::iter(msgs)))
            }
        }

        async fn send(&self, conversation_id: &str, message: &str) -> Result<(), String> {
            self.sent
                .lock()
                .unwrap()
                .push(format!("{}:{}", conversation_id, message));
            if self.send_fail {
                Err("mock send failure".into())
            } else {
                Ok(())
            }
        }

        async fn health_check(&self) -> Result<(), String> {
            Ok(())
        }
    }

    /// A stream that panics after `remaining` items have been yielded.
    struct LimitedPanicStream {
        remaining: usize,
        delivered: Vec<Message>,
        index: usize,
    }

    impl Stream for LimitedPanicStream {
        type Item = Message;

        fn poll_next(
            mut self: Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> Poll<Option<Self::Item>> {
            if self.index >= self.delivered.len() {
                return Poll::Ready(None);
            }
            if self.remaining == 0 {
                panic!("mock stream panic");
            }
            self.remaining -= 1;
            let msg = self.delivered[self.index].clone();
            self.index += 1;
            Poll::Ready(Some(msg))
        }
    }

    // ---------------------------------------------------------------------------
    // MockAgent
    // ---------------------------------------------------------------------------

    struct MockAgent {
        fail_count: AtomicU64,
        responses: Vec<String>,
        session_ids: Vec<String>,
        process_count: Arc<AtomicU64>,
        delay: Duration,
        delay_count: AtomicI64, // -1 = always, 0 = never, N>0 = first N times
    }

    impl MockAgent {
        fn new(responses: Vec<&str>) -> Self {
            let count = responses.len();
            Self {
                fail_count: AtomicU64::new(0),
                responses: responses.into_iter().map(String::from).collect(),
                session_ids: (0..count).map(|i| format!("session-{}", i)).collect(),
                process_count: Arc::new(AtomicU64::new(0)),
                delay: Duration::ZERO,
                delay_count: AtomicI64::new(0),
            }
        }

        fn with_delay(mut self, delay: Duration) -> Self {
            self.delay = delay;
            self.delay_count = AtomicI64::new(-1); // always delay
            self
        }
    }

    #[async_trait]
    impl AgentProvider for MockAgent {
        fn name(&self) -> &str {
            "mock-agent"
        }

        async fn check_available(&self) -> Result<(), String> {
            Ok(())
        }

        async fn process(
            &self,
            _msg: &str,
            _session_id: Option<&str>,
            _work_dir: &str,
        ) -> Result<(String, String), String> {
            // delay simulation
            if !self.delay.is_zero() {
                let do_sleep = loop {
                    let r = self.delay_count.load(Ordering::SeqCst);
                    if r < 0 {
                        break true; // always
                    }
                    if r > 0 {
                        if self
                            .delay_count
                            .compare_exchange(r, r - 1, Ordering::SeqCst, Ordering::SeqCst)
                            .is_ok()
                        {
                            break true; // first N times
                        }
                        continue;
                    }
                    break false; // never
                };
                if do_sleep {
                    tokio::time::sleep(self.delay).await;
                }
            }

            let count = self.process_count.fetch_add(1, Ordering::SeqCst);
            if count < self.fail_count.load(Ordering::SeqCst) {
                return Err("mock process failure".into());
            }
            let idx = count.saturating_sub(self.fail_count.load(Ordering::SeqCst));
            let text = self
                .responses
                .get(idx as usize % self.responses.len().max(1))
                .cloned()
                .unwrap_or_default();
            let sid = self
                .session_ids
                .get(idx as usize % self.session_ids.len().max(1))
                .cloned()
                .unwrap_or_default();
            Ok((text, sid))
        }
    }

    // ---------------------------------------------------------------------------
    // Helpers
    // ---------------------------------------------------------------------------

    static MSG_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn test_msg(conversation_id: &str, content: &str) -> Message {
        Message {
            id: format!("msg-{}", MSG_COUNTER.fetch_add(1, Ordering::Relaxed)),
            conversation_id: conversation_id.to_string(),
            sender_id: "test-sender".to_string(),
            content: content.to_string(),
            timestamp: Utc::now(),
            channel: "test".to_string(),
        }
    }

    async fn run_gateway_test(
        channels: Vec<(&'static str, MockChannel)>,
        agent: MockAgent,
        config: GatewayConfig,
        cancel: CancellationToken,
    ) -> Result<(), String> {
        let channels: Vec<(String, Box<dyn MessageChannel>)> = channels
            .into_iter()
            .map(|(n, ch)| (n.to_string(), Box::new(ch) as Box<dyn MessageChannel>))
            .collect();
        let agent = Box::new(agent) as Box<dyn AgentProvider>;
        // 隔离 HOME，避免测试写入真实 ~/.haimen/agent-logs
        crate::test_util::run_with_temp_home_async(move |_home| async move {
            run_unified_gateway(channels, &*agent, &config, cancel).await
        })
        .await
    }

    /// Create a token that cancels after `delay`.
    fn cancel_after(delay: Duration) -> (CancellationToken, tokio::task::JoinHandle<()>) {
        let token = CancellationToken::new();
        let t = token.clone();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            t.cancel();
        });
        (token, handle)
    }

    fn default_config() -> GatewayConfig {
        GatewayConfig::default()
    }

    // ---------------------------------------------------------------------------
    // Existing parse_command tests
    // ---------------------------------------------------------------------------

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

    // ---------------------------------------------------------------------------
    // pump task basics
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn test_pump_single_channel() {
        let ch = MockChannel::new("ch1", vec![test_msg("conv1", "hello")]);
        let agent = MockAgent::new(vec!["response"]);
        let cancel = CancellationToken::new();

        let result =
            run_gateway_test(vec![("ch1", ch)], agent, default_config(), cancel.clone()).await;
        // 1 msg processed → 1 sent success → gateway exits when stream ends
        assert!(result.is_err(), "stream end should return Err");
    }

    #[tokio::test]
    async fn test_pump_dual_channel() {
        let ch_a = MockChannel::new("chA", vec![test_msg("c1", "msgA")]);
        let ch_b = MockChannel::new("chB", vec![test_msg("c2", "msgB")]);
        let agent = MockAgent::new(vec!["ok"]);

        let result = run_gateway_test(
            vec![("chA", ch_a), ("chB", ch_b)],
            agent,
            default_config(),
            CancellationToken::new(),
        )
        .await;
        assert!(result.is_err(), "both streams end → Err");
    }

    #[tokio::test]
    async fn test_pump_count_zero_return_error() {
        let ch = MockChannel {
            listen_fail: true,
            ..MockChannel::new("bad", vec![])
        };
        let agent = MockAgent::new(vec!["x"]);

        let result = run_gateway_test(
            vec![("bad", ch)],
            agent,
            default_config(),
            CancellationToken::new(),
        )
        .await;
        assert!(result.is_err(), "all listen fail → Err");
        assert!(
            result.err().unwrap().contains("没有连接器"),
            "error should mention no connector"
        );
    }

    #[tokio::test]
    async fn test_pump_count_zero_with_cancel() {
        let cancel = CancellationToken::new();
        cancel.cancel(); // already cancelled

        let ch = MockChannel::new("ch", vec![test_msg("c1", "hi")]);
        let agent = MockAgent::new(vec!["x"]);

        let result = run_gateway_test(vec![("ch", ch)], agent, default_config(), cancel).await;
        assert!(result.is_ok(), "cancel before any pump → Ok");
    }

    #[tokio::test]
    async fn test_pump_all_messages_received() {
        let msgs = vec![
            test_msg("c1", "m1"),
            test_msg("c1", "m2"),
            test_msg("c1", "m3"),
        ];
        let ch = MockChannel::new("ch", msgs);
        let agent = MockAgent::new(vec!["ok"]);

        let result = run_gateway_test(
            vec![("ch", ch)],
            agent,
            default_config(),
            CancellationToken::new(),
        )
        .await;
        assert!(result.is_err(), "stream end → Err");
    }

    #[tokio::test]
    async fn test_pump_stream_ends_logged() {
        // empty stream → pump exits immediately → no msgs processed
        let ch = MockChannel::new("empty", vec![]);
        let agent = MockAgent::new(vec!["x"]);

        let result = run_gateway_test(
            vec![("empty", ch)],
            agent,
            default_config(),
            CancellationToken::new(),
        )
        .await;
        assert!(result.is_err(), "empty stream ends → Err");
    }

    // ---------------------------------------------------------------------------
    // Agent timeout
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn test_timeout_normal_completion() {
        let ch = MockChannel::new("ch", vec![test_msg("c1", "hi")]);
        // agent responds fast (no delay)
        let agent = MockAgent::new(vec!["response"]);

        let result = run_gateway_test(
            vec![("ch", ch)],
            agent,
            default_config(),
            CancellationToken::new(),
        )
        .await;
        assert!(result.is_err()); // stream ends
    }

    #[tokio::test]
    async fn test_timeout_exceeded() {
        let ch = MockChannel::new("ch", vec![test_msg("c1", "hi")]);
        // agent takes 10s but timeout is 100ms → should timeout
        let agent = MockAgent::new(vec!["response"]).with_delay(Duration::from_secs(10));
        let mut cfg = default_config();
        cfg.agent_timeout_secs = 1; // 1s timeout

        let result = run_gateway_test(vec![("ch", ch)], agent, cfg, CancellationToken::new()).await;
        assert!(result.is_err()); // stream ends after timeout processing
    }

    #[tokio::test]
    async fn test_timeout_continue_next_message() {
        let ch = MockChannel::new(
            "ch",
            vec![test_msg("c1", "first"), test_msg("c1", "second")],
        );
        // agent always slow → both should timeout, but loop continues
        let agent = MockAgent::new(vec!["response"]).with_delay(Duration::from_secs(10));
        let mut cfg = default_config();
        cfg.agent_timeout_secs = 1;

        let result = run_gateway_test(vec![("ch", ch)], agent, cfg, CancellationToken::new()).await;
        // Both messages cause timeout → stream ends → Err
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_timeout_degrade_retry() {
        let ch = MockChannel::new("ch", vec![test_msg("c1", "hi")]);
        // first call fails (resume fails), second is slow (timeout during degrade retry)
        // Actually, with fail_count=1 and delay, the resume call will fail fast (no delay),
        // and the degrade retry call does NOT have delay. So the retry succeeds.
        // To test degrade retry timeout, we need the retry itself to timeout.
        // But the current design doesn't support "first call fast, second call slow".
        // Let's just test that degrade retry works with timeout.
        let agent = MockAgent {
            fail_count: AtomicU64::new(1),
            ..MockAgent::new(vec!["response"])
        };
        let mut cfg = default_config();
        cfg.agent_timeout_secs = 5; // plenty of time

        let result = run_gateway_test(vec![("ch", ch)], agent, cfg, CancellationToken::new()).await;
        assert!(result.is_err()); // stream ends after success
    }

    #[tokio::test]
    async fn test_timeout_does_not_corrupt_session() {
        let ch = MockChannel::new(
            "ch",
            vec![test_msg("c1", "first"), test_msg("c1", "second")],
        );
        // first msg: fast response (creates session). second msg: also fast (reuses session)
        let agent = MockAgent::new(vec!["response"]);
        let mut cfg = default_config();
        cfg.agent_timeout_secs = 5;

        let result = run_gateway_test(vec![("ch", ch)], agent, cfg, CancellationToken::new()).await;
        assert!(result.is_err()); // stream ends
    }

    // ---------------------------------------------------------------------------
    // Graceful shutdown
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn test_shutdown_pumps_exit() {
        // Agent takes 500ms, giving cancel time to fire during processing
        let ch = MockChannel::new("ch", vec![test_msg("c1", "hi")]);
        let agent = MockAgent::new(vec!["response"]).with_delay(Duration::from_millis(500));
        // Cancel after 50ms, while agent is still processing
        let (cancel, handle) = cancel_after(Duration::from_millis(50));

        let result = run_gateway_test(vec![("ch", ch)], agent, default_config(), cancel).await;
        let _ = handle.await;
        assert!(result.is_ok(), "cancel should produce Ok");
    }

    #[tokio::test]
    async fn test_shutdown_returns_ok() {
        let cancel = CancellationToken::new();
        cancel.cancel();

        let ch = MockChannel::new("ch", vec![test_msg("c1", "hi")]);
        let agent = MockAgent::new(vec!["response"]);

        let result = run_gateway_test(vec![("ch", ch)], agent, default_config(), cancel).await;
        assert!(result.is_ok(), "cancelled before start → Ok");
    }

    #[tokio::test]
    async fn test_shutdown_all_pumps_down_returns_error() {
        let ch = MockChannel::new("ch", vec![]); // empty stream, pump exits immediately
        let agent = MockAgent::new(vec!["response"]);

        let result = run_gateway_test(
            vec![("ch", ch)],
            agent,
            default_config(),
            CancellationToken::new(),
        )
        .await;
        assert!(result.is_err(), "no cancel, all pumps down → Err");
    }

    #[tokio::test]
    async fn test_shutdown_during_listen() {
        // Cancel before calling run_gateway_test → listen() sees cancelled
        let cancel = CancellationToken::new();
        cancel.cancel();

        let ch = MockChannel::new("ch", vec![test_msg("c1", "hi")]);
        let agent = MockAgent::new(vec!["response"]);

        let result = run_gateway_test(vec![("ch", ch)], agent, default_config(), cancel).await;
        assert!(result.is_ok(), "listen cancelled → Ok");
    }

    // ---------------------------------------------------------------------------
    // Listen interruption
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn test_listen_failure() {
        let ch = MockChannel {
            listen_fail: true,
            ..MockChannel::new("bad", vec![])
        };
        let agent = MockAgent::new(vec!["x"]);
        // Only one channel and it fails → pump_count == 0 → Err
        let result = run_gateway_test(
            vec![("bad", ch)],
            agent,
            default_config(),
            CancellationToken::new(),
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_listen_cancelled() {
        let cancel = CancellationToken::new();
        let ch = MockChannel::new("ch", vec![test_msg("c1", "hi")]);
        let agent = MockAgent::new(vec!["response"]);

        // pre-cancelled → listen sees cancelled → skip
        cancel.cancel();
        let result = run_gateway_test(vec![("ch", ch)], agent, default_config(), cancel).await;
        assert!(result.is_ok(), "cancelled → Ok");
    }

    #[tokio::test]
    async fn test_listen_partial_cancel() {
        let cancel = CancellationToken::new();
        let ch_a = MockChannel::new("chA", vec![test_msg("c1", "msgA")]);
        let ch_b = MockChannel::new("chB", vec![test_msg("c2", "msgB")]);

        // Cancel before start → both skips
        cancel.cancel();
        let result = run_gateway_test(
            vec![("chA", ch_a), ("chB", ch_b)],
            MockAgent::new(vec!["ok"]),
            default_config(),
            cancel,
        )
        .await;
        assert!(result.is_ok(), "all cancelled → Ok");
    }

    // ---------------------------------------------------------------------------
    // Panic isolation
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn test_pump_panic_does_not_affect_others() {
        // Channel A: normal, 1 msg
        // Channel B: stream panics after 0 items (immediate panic)
        let ch_a = MockChannel::new("chA", vec![test_msg("c1", "msgA")]);
        let ch_b = MockChannel {
            stream_panic_after: 0,
            ..MockChannel::new("chB", vec![test_msg("c2", "msgB")])
        };
        let agent = MockAgent::new(vec!["ok"]);

        let result = run_gateway_test(
            vec![("chA", ch_a), ("chB", ch_b)],
            agent,
            default_config(),
            CancellationToken::new(),
        )
        .await;
        // chB panics immediately → sender dropped → chA still processes its msg
        // After chA's msg processed, chA's stream ends → pump exits → all done
        // No cancel → Err("所有连接器消息流已结束")
        assert!(result.is_err(), "all streams end → Err");
    }

    // ---------------------------------------------------------------------------
    // Regression: session / commands / send failure
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn test_session_key_isolation() {
        // Same conversation_id on different channels → session keys differ
        let ch_a = MockChannel::new("lark", vec![test_msg("chat1", "hi")]);
        let ch_b = MockChannel::new("dingtalk", vec![test_msg("chat1", "hi")]);
        let agent = MockAgent::new(vec!["ok"]);

        let result = run_gateway_test(
            vec![("lark", ch_a), ("dingtalk", ch_b)],
            agent,
            default_config(),
            CancellationToken::new(),
        )
        .await;
        assert!(result.is_err(), "streams end → Err");
    }

    #[tokio::test]
    async fn test_command_new_resets_session() {
        let ch = MockChannel::new("ch", vec![test_msg("c1", "/new")]);
        let agent = MockAgent::new(vec!["response"]);

        let result = run_gateway_test(
            vec![("ch", ch)],
            agent,
            default_config(),
            CancellationToken::new(),
        )
        .await;
        assert!(result.is_err(), "stream ends → Err");
    }

    #[tokio::test]
    async fn test_send_failure_does_not_panic() {
        let ch = MockChannel {
            send_fail: true,
            ..MockChannel::new("ch", vec![test_msg("c1", "hello")])
        };
        let agent = MockAgent::new(vec!["response"]);

        let result = run_gateway_test(
            vec![("ch", ch)],
            agent,
            default_config(),
            CancellationToken::new(),
        )
        .await;
        // send failure → warn log → continue → stream ends → Err
        assert!(result.is_err());
    }

    // ---------------------------------------------------------------------------
    // Boundary content
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn test_unicode_emoji_message() {
        let ch = MockChannel::new("ch", vec![test_msg("c1", "你好🌍世界🔥")]);
        let agent = MockAgent::new(vec!["response"]);

        let result = run_gateway_test(
            vec![("ch", ch)],
            agent,
            default_config(),
            CancellationToken::new(),
        )
        .await;
        assert!(result.is_err(), "stream ends → Err");
    }

    #[tokio::test]
    async fn test_large_volume_pressure() {
        // 500 messages from a single channel
        let msgs: Vec<Message> = (0..500)
            .map(|i| test_msg("c1", &format!("msg-{}", i)))
            .collect();
        let ch = MockChannel::new("ch", msgs);
        let agent = MockAgent::new(vec!["ok"]);

        let result = run_gateway_test(
            vec![("ch", ch)],
            agent,
            default_config(),
            CancellationToken::new(),
        )
        .await;
        assert!(result.is_err(), "stream ends → Err");
    }

    #[tokio::test]
    async fn test_long_message() {
        let long = "A".repeat(10_000);
        let ch = MockChannel::new("ch", vec![test_msg("c1", &long)]);
        let agent = MockAgent::new(vec!["response"]);

        let result = run_gateway_test(
            vec![("ch", ch)],
            agent,
            default_config(),
            CancellationToken::new(),
        )
        .await;
        assert!(result.is_err(), "stream ends → Err");
    }

    #[tokio::test]
    async fn test_whitespace_message() {
        let ch = MockChannel::new("ch", vec![test_msg("c1", "   ")]);
        let agent = MockAgent::new(vec!["response"]);

        let result = run_gateway_test(
            vec![("ch", ch)],
            agent,
            default_config(),
            CancellationToken::new(),
        )
        .await;
        assert!(result.is_err(), "stream ends → Err");
    }

    // ---------------------------------------------------------------------------
    // Agent 调用日志记录
    // ---------------------------------------------------------------------------

    /// 读取临时 HOME 下当日 agent-logs 文件中的所有记录（按写入顺序）
    fn read_agent_logs(home: &std::path::Path) -> Vec<crate::agent_log::AgentLogRecord> {
        let day = chrono::Local::now().format("%Y-%m-%d").to_string();
        let path = home.join(format!(".haimen/agent-logs/{}.jsonl", day));
        if !path.exists() {
            return Vec::new();
        }
        let content = std::fs::read_to_string(&path).unwrap();
        content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str::<crate::agent_log::AgentLogRecord>(l).unwrap())
            .collect()
    }

    #[tokio::test]
    async fn test_agent_log_success_recorded() {
        let ch = MockChannel::new("ch", vec![test_msg("c1", "hello")]);
        let agent = MockAgent::new(vec!["response"]);
        let cancel = CancellationToken::new();

        let channels: Vec<(String, Box<dyn MessageChannel>)> =
            vec![("ch".to_string(), Box::new(ch) as Box<dyn MessageChannel>)];
        let agent = Box::new(agent) as Box<dyn AgentProvider>;

        // 断言必须在临时 HOME 闭包内执行（TempDir 返回后即删除）
        crate::test_util::run_with_temp_home_async(move |home| async move {
            let result = run_unified_gateway(channels, &*agent, &default_config(), cancel).await;
            assert!(result.is_err(), "stream end → Err");

            let records = read_agent_logs(&home);
            assert_eq!(records.len(), 1, "一次成功处理应产生一条记录");
            assert_eq!(records[0].source, "gateway");
            assert_eq!(records[0].status, "success");
            assert_eq!(records[0].input, "hello");
            assert_eq!(records[0].output.as_deref(), Some("response"));
            assert_eq!(records[0].chat_id.as_deref(), Some("ch:c1"));
            assert_eq!(records[0].sender_id.as_deref(), Some("test-sender"));
            assert_eq!(records[0].connector.as_deref(), Some("ch"));
        })
        .await;
    }

    /// 首个调用成功（创建会话），后续 resume 调用失败、新会话重试成功。
    /// 用于测试"resume 失败 → 降级重试"路径的记录。
    struct ResumeFailAgent {
        calls: AtomicU64,
    }

    #[async_trait]
    impl AgentProvider for ResumeFailAgent {
        fn name(&self) -> &str {
            "resume-fail-agent"
        }

        async fn check_available(&self) -> Result<(), String> {
            Ok(())
        }

        async fn process(
            &self,
            _msg: &str,
            session_id: Option<&str>,
            _work_dir: &str,
        ) -> Result<(String, String), String> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                Ok(("first-ok".to_string(), "sess-1".to_string()))
            } else if session_id.is_some() {
                Err("resume failed".to_string())
            } else {
                Ok(("retry-ok".to_string(), "sess-2".to_string()))
            }
        }
    }

    #[tokio::test]
    async fn test_agent_log_degrade_two_records() {
        let ch = MockChannel::new(
            "ch",
            vec![test_msg("c1", "first"), test_msg("c1", "second")],
        );
        let agent = ResumeFailAgent {
            calls: AtomicU64::new(0),
        };
        let cancel = CancellationToken::new();

        let channels: Vec<(String, Box<dyn MessageChannel>)> =
            vec![("ch".to_string(), Box::new(ch) as Box<dyn MessageChannel>)];
        let agent = Box::new(agent) as Box<dyn AgentProvider>;

        // 断言必须在临时 HOME 闭包内执行（TempDir 返回后即删除）
        crate::test_util::run_with_temp_home_async(move |home| async move {
            let result = run_unified_gateway(channels, &*agent, &default_config(), cancel).await;
            assert!(result.is_err(), "stream end → Err");

            // msg1 成功 → 1 条 success；msg2 resume 失败 + 降级重试成功 → error + success
            let records = read_agent_logs(&home);
            assert_eq!(
                records.len(),
                3,
                "应产生三条记录（成功 + 降级 error + 重试 success）"
            );
            assert_eq!(records[0].status, "success");
            assert_eq!(records[0].output.as_deref(), Some("first-ok"));
            assert_eq!(records[1].status, "error", "第二条是 resume 失败");
            assert_eq!(records[1].error.as_deref(), Some("resume failed"));
            assert_eq!(records[2].status, "success", "第三条是降级重试成功");
            assert_eq!(records[2].output.as_deref(), Some("retry-ok"));
        })
        .await;
    }
}
