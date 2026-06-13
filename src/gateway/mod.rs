//! AI 网关模块
//!
//! 预留 AI 处理管线模块，未来将支持从飞书（或其他渠道）接收消息、
//! 经过 AI 处理（LLM 调用、Prompt 链等）、将结果返回渠道等能力。

/// 网关状态
pub struct GatewayStatus {
    /// 是否启用 AI 处理
    pub enabled: bool,
    /// AI 提供商
    pub provider: Option<String>,
    /// 活跃连接数
    pub active_connections: u32,
}

/// 获取网关状态
pub fn status() -> GatewayStatus {
    let config = crate::config::settings::load_settings()
        .ok()
        .flatten()
        .unwrap_or_default();

    GatewayStatus {
        enabled: config.gateway.enabled.unwrap_or(false),
        provider: config.gateway.provider.clone(),
        active_connections: 0,
    }
}
