mod ota;
mod protocol;
pub mod types;
mod ws;

pub use protocol::*;
pub use types::*;

use axum::Router;

/// 向 axum Router 注入 xiaozhi 设备通信端点
///
/// 添加两条路由：
/// - `POST /xiaozhi/ota/` — OTA 握手，返回 WebSocket URL 和音频参数
/// - `GET  /xiaozhi/ws`   — WebSocket 升级端点
///
/// 接受任意 State 类型（`S: Clone + Send + Sync + 'static`），
/// 因为 xiaozhi 处理器不依赖外部状态。
pub fn add_routes<S: Clone + Send + Sync + 'static>(router: Router<S>) -> Router<S> {
    router
        .route("/xiaozhi/ota/", axum::routing::post(ota::handle_ota))
        .route("/xiaozhi/ws", axum::routing::get(ws::handle_ws_upgrade))
}
