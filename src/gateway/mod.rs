pub mod channel;
pub mod chat_loop;
pub mod model;
pub mod provider;
pub mod session;
pub mod webhook;

use crate::agents::claude_code::agent::ClaudeAgent;
use crate::config::settings::load_settings;
use crate::connectors::dingtalk::channel::DingTalkChannel;
use crate::gateway::channel::MessageChannel;
use crate::gateway::provider::AgentProvider;
use futures_util::StreamExt;
use haimen_lark::LarkChannel;

/// 构建后的连接器列表类型
type ConnectorVec = Vec<(String, Box<dyn MessageChannel>)>;

/// 根据配置构建所有启用的连接器
pub fn build_connectors(
    config: &crate::config::settings::AppConfig,
) -> Result<ConnectorVec, String> {
    let mut channels: ConnectorVec = Vec::new();

    // Lark
    if let Some(lark_cfg) = &config.connectors.lark {
        if lark_cfg.enabled {
            channels.push((
                "lark".to_string(),
                Box::new(LarkChannel::new(&lark_cfg.lark_cli_path)) as Box<dyn MessageChannel>,
            ));
        }
    }

    // DingTalk
    if let Some(dt_cfg) = &config.connectors.dingtalk {
        if dt_cfg.enabled {
            let dingtalk_cfg: crate::connectors::dingtalk::config::DingTalkConfig =
                dt_cfg.clone().into();
            channels.push((
                "dingtalk".to_string(),
                Box::new(DingTalkChannel::new(dingtalk_cfg)) as Box<dyn MessageChannel>,
            ));
        }
    }

    Ok(channels)
}

/// 根据配置构建 Agent
pub fn build_agent(
    config: &crate::config::settings::AppConfig,
) -> Result<Box<dyn AgentProvider>, String> {
    let agent_name = config.gateway.agent.as_deref().unwrap_or("claude-code");

    match agent_name {
        "claude-code" => Ok(Box::new(ClaudeAgent)),
        other => Err(format!("不支持的 AI Agent: {}", other)),
    }
}

/// 统一入口：启动所有启用的连接器 + Agent
///
/// 流程：
/// 1. 构建连接器
/// 2. 构建 Agent
/// 3. 各连接器健康检查（并行，失败的跳过）
/// 4. 各连接器 listen（并行，失败的跳过）
/// 5. 运行多连接器事件循环
pub async fn start_all() -> Result<(), String> {
    let config = load_settings().ok().flatten().unwrap_or_default();

    let all_connectors = build_connectors(&config)?;
    let agent = build_agent(&config)?;

    if all_connectors.is_empty() {
        tracing::info!("没有启用的连接器");
        return Ok(());
    }

    // 并行健康检查，收集健康的连接器名
    let healthy: Vec<String> =
        futures_util::future::join_all(all_connectors.iter().map(|(name, ch)| {
            let name = name.clone();
            async move {
                match ch.health_check().await {
                    Ok(_) => {
                        tracing::info!(connector = %name, "健康检查通过");
                        Some(name)
                    }
                    Err(e) => {
                        tracing::warn!(connector = %name, error = %e, "健康检查失败，跳过");
                        None
                    }
                }
            }
        }))
        .await
        .into_iter()
        .flatten()
        .collect();

    if healthy.is_empty() {
        tracing::warn!("所有连接器健康检查均失败，无法启动网关");
        return Ok(());
    }

    // 对健康的连接器执行 listen，收集成功的 stream
    let config = load_settings().ok().flatten().unwrap_or_default();

    let mut channels: ConnectorVec = Vec::new();
    let mut streams = Vec::new();

    for name in &healthy {
        let ch = match name.as_str() {
            "lark" => {
                let cfg = config
                    .connectors
                    .lark
                    .as_ref()
                    .ok_or_else(|| "Lark 配置不存在".to_string())?;
                Box::new(LarkChannel::new(&cfg.lark_cli_path)) as Box<dyn MessageChannel>
            }
            "dingtalk" => {
                let cfg = config
                    .connectors
                    .dingtalk
                    .clone()
                    .ok_or_else(|| "DingTalk 配置不存在".to_string())?;
                Box::new(DingTalkChannel::new(cfg.into())) as Box<dyn MessageChannel>
            }
            other => return Err(format!("不支持的连接器: {}", other)),
        };

        match ch.listen().await {
            Ok(stream) => {
                let cn = name.clone();
                let tagged = stream.map(move |msg| (cn.clone(), msg));
                streams.push(tagged);
                channels.push((name.clone(), ch));
            }
            Err(e) => {
                tracing::warn!(connector = %name, error = %e, "listen 失败，跳过");
            }
        }
    }

    if streams.is_empty() {
        tracing::warn!("没有连接器成功启动消息流");
        return Ok(());
    }

    agent.check_available().await?;

    tracing::info!(
        "网关已启动，活跃连接器: {:?}",
        channels
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<&str>>()
    );

    chat_loop::run_unified_gateway(channels, &*agent, &config.gateway).await
}

/// 启动网关监听（单连接器模式，取第一个启用的连接器）
pub async fn listen() -> Result<(), String> {
    let config = load_settings().ok().flatten().unwrap_or_default();

    let mut channels = build_connectors(&config)?;
    if channels.is_empty() {
        return Err("没有启用的连接器".to_string());
    }

    let (name, channel) = channels.remove(0);
    tracing::info!(connector = %name, "单连接器模式");

    let agent = build_agent(&config)?;
    agent.check_available().await?;

    chat_loop::run_chat_loop(&*channel, &*agent, &config.gateway).await
}

/// 启动网关监听（Echo 模式，取第一个启用的连接器）
pub async fn listen_echo() -> Result<(), String> {
    let config = load_settings().ok().flatten().unwrap_or_default();

    let mut channels = build_connectors(&config)?;
    if channels.is_empty() {
        return Err("没有启用的连接器".to_string());
    }

    let (_, channel) = channels.remove(0);
    chat_loop::run_echo_loop(&*channel).await
}

/// Echo 模式：启动所有启用的连接器，直接 echo 不经过 Agent
pub async fn start_echo() -> Result<(), String> {
    let config = load_settings().ok().flatten().unwrap_or_default();

    let connectors = build_connectors(&config)?;
    if connectors.is_empty() {
        tracing::info!("没有启用的连接器");
        return Ok(());
    }

    let mut streams = Vec::new();
    for (name, ch) in &connectors {
        match ch.listen().await {
            Ok(stream) => {
                let cn = name.clone();
                let tagged = stream.map(move |msg| (cn.clone(), msg));
                streams.push(tagged);
            }
            Err(e) => {
                tracing::warn!(connector = %name, error = %e, "listen 失败，跳过");
            }
        }
    }

    if streams.is_empty() {
        tracing::warn!("没有连接器成功启动");
        return Ok(());
    }

    let mut merged = futures_util::stream::select_all(streams);
    while let Some((connector_name, message)) = merged.next().await {
        if let Some((_, ch)) = connectors.iter().find(|(n, _)| n == &connector_name) {
            let echo = format!("🔄 Echo: {}", message.content);
            let _ = ch.send(&message.conversation_id, &echo).await;
        }
    }

    Err("所有消息流意外结束".to_string())
}
