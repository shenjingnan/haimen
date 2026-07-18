mod ota;
mod protocol;
pub mod strategy;
pub mod types;
mod ws;

pub use protocol::*;
pub use strategy::{EchoStrategy, ResponseStrategy};
pub use types::*;

use std::sync::Arc;

use axum::Router;

/// 向 axum Router 注入 xiaozhi 设备通信端点
///
/// 添加两条路由：
/// - `POST /xiaozhi/ota/` — OTA 握手，返回 WebSocket URL 和音频参数
/// - `GET  /xiaozhi/ws`   — WebSocket 升级端点
///
/// # 参数
///
/// * `router` — 待扩展的 axum Router
/// * `strategy` — WebSocket 响应策略，控制设备录音结束后的回放行为
///
/// 接受任意 State 类型（`S: Clone + Send + Sync + 'static`），
/// 因为 xiaozhi 处理器不依赖外部状态。
pub fn add_routes<S: Clone + Send + Sync + 'static>(
    router: Router<S>,
    strategy: Arc<dyn ResponseStrategy>,
) -> Router<S> {
    router
        .route("/xiaozhi/ota/", axum::routing::post(ota::handle_ota))
        .route(
            "/xiaozhi/ws",
            axum::routing::get(move |ws, headers| {
                ws::handle_ws_upgrade(ws, headers, strategy.clone())
            }),
        )
}
