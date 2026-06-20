use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use tracing;

use crate::gateway::webhook::WebhookState;

/// GitHub Webhook 处理器
///
/// 由 web::start() 中的 Router 注册（带 WebhookState），
/// 收到 POST /webhook/github 时调用。
pub async fn handle_github_webhook(
    State(state): State<WebhookState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let connector = match &state.github {
        Some(c) => c,
        None => {
            tracing::warn!("收到 GitHub webhook 但未配置 GitHubConnector");
            return StatusCode::NOT_FOUND;
        }
    };

    match connector.handle(&body, &headers).await {
        Ok(_) => StatusCode::OK,
        Err(e) => {
            tracing::error!(error = %e, "GitHub webhook 处理失败");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}
