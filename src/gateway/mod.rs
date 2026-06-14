pub mod chat_loop;

use crate::feishu::bridge::LarkCliBridge;

/// 网关状态
pub struct GatewayStatus {
    pub enabled: bool,
    pub provider: Option<String>,
    pub active_connections: u32,
    pub mcp_servers: Vec<String>,
}

/// 获取网关状态
pub fn status() -> GatewayStatus {
    let config = crate::config::settings::load_settings()
        .ok()
        .flatten()
        .unwrap_or_default();

    let mcp_servers: Vec<String> = config.gateway.mcp_servers.keys().cloned().collect();

    GatewayStatus {
        enabled: config.gateway.enabled.unwrap_or(false),
        provider: config.gateway.provider.clone(),
        active_connections: 0,
        mcp_servers,
    }
}

/// 启动网关监听
///
/// 监听飞书消息 → claude --print 处理 → 结果回飞书
pub async fn listen() -> Result<(), String> {
    let config = crate::config::settings::load_settings()
        .ok()
        .flatten()
        .unwrap_or_default();

    // 创建飞书桥接
    let feishu_bridge = LarkCliBridge::new(&config.feishu.lark_cli_path);

    // 运行编排循环
    chat_loop::run_chat_loop(&feishu_bridge).await
}
