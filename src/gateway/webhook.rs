use std::sync::Arc;

use async_trait::async_trait;
use axum::http::HeaderMap;

/// Webhook 处理器接口
///
/// handle() 全权负责验证、解析、触发和回复。
/// 调用者（webhook 路由层）只关心：
/// - Ok(WebhookResult { triggered: true }) → 200
/// - Ok(WebhookResult { triggered: false }) → 200（无动作，不重试）
/// - Err(msg) → 500（外部系统会重试）
#[async_trait]
pub trait WebhookHandler: Send + Sync {
    fn name(&self) -> &str;

    async fn handle(&self, body: &[u8], headers: &HeaderMap) -> Result<WebhookResult, String>;
}

/// Webhook 处理结果
pub struct WebhookResult {
    /// 是否触发了 Agent 动作（用于日志/监控）
    pub triggered: bool,
}

/// 持有一组可选的 Webhook 处理器，注入 web 服务器路由
///
/// 定义在 gateway 层而非 web 层，避免循环依赖（web → gateway → web）。
/// 依赖方向始终单向：web → gateway → connectors。
#[derive(Clone)]
pub struct WebhookState {
    pub github: Option<Arc<dyn WebhookHandler>>,
}
