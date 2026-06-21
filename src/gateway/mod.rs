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
use haimen_lark::LarkChannel;

/// 网关状态
pub struct GatewayStatus {
    pub enabled: bool,
    pub provider: Option<String>,
    pub active_connections: u32,
    pub mcp_servers: Vec<String>,
}

/// 获取网关状态
pub fn status() -> GatewayStatus {
    let config = load_settings().ok().flatten().unwrap_or_default();

    let mcp_servers: Vec<String> = config.gateway.mcp_servers.keys().cloned().collect();

    GatewayStatus {
        enabled: config.gateway.enabled.unwrap_or(false),
        provider: config.gateway.provider.clone(),
        active_connections: 0,
        mcp_servers,
    }
}

/// 启动网关监听（IM 通道）
///
/// 根据配置动态构造 Channel + Agent，运行通用编排循环。
pub async fn listen() -> Result<(), String> {
    let config = load_settings().ok().flatten().unwrap_or_default();

    // 根据配置构造 IM 通道
    let channel: Box<dyn MessageChannel> = match config.gateway.channel.as_str() {
        "lark" => Box::new(LarkChannel::new(&config.feishu.lark_cli_path)),
        "dingtalk" => {
            let dt_config = config
                .dingtalk
                .ok_or_else(|| "未配置 [dingtalk] 节".to_string())?;
            Box::new(DingTalkChannel::new(dt_config))
        }
        other => return Err(format!("不支持的 IM 通道: {}", other)),
    };

    // 根据配置构造 Agent（当前仅支持 claude-code）
    let agent: Box<dyn AgentProvider> = match config.gateway.provider.as_deref() {
        Some("mcp") => {
            return Err("MCP Agent 暂未在 listen 模式下支持".to_string());
        }
        _ => Box::new(ClaudeAgent),
    };

    chat_loop::run_chat_loop(&*channel, &*agent, &config.gateway).await
}

/// 启动网关监听（Echo 模式）
///
/// 只启动 Channel，收到消息后直接 echo 回去，不经过 Agent 处理。
/// 用于验证通道连通性和消息格式。
pub async fn listen_echo() -> Result<(), String> {
    let config = load_settings().ok().flatten().unwrap_or_default();

    let channel: Box<dyn MessageChannel> = match config.gateway.channel.as_str() {
        "lark" => Box::new(LarkChannel::new(&config.feishu.lark_cli_path)),
        "dingtalk" => {
            let dt_config = config
                .dingtalk
                .ok_or_else(|| "未配置 [dingtalk] 节".to_string())?;
            Box::new(DingTalkChannel::new(dt_config))
        }
        other => return Err(format!("不支持的 IM 通道: {}", other)),
    };

    chat_loop::run_echo_loop(&*channel).await
}
