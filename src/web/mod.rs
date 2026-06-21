pub mod api;
pub mod r#static;

use std::net::SocketAddr;

use crate::gateway::webhook::WebhookState;

/// 服务器配置
pub struct ServeConfig {
    pub host: String,
    pub port: u16,
}

/// 启动 HTTP 服务器
///
/// - `config`: 服务器地址配置
/// - `webhook_state`: 可选的 Webhook 处理器
pub async fn start(config: ServeConfig, webhook_state: Option<WebhookState>) -> Result<(), String> {
    use haimen_xiaozhi;

    // 构建路由（WebhookState 分支需要用 .with_state() 传递 state）
    let app = if let Some(state) = webhook_state {
        haimen_xiaozhi::add_routes(
            axum::Router::new()
                .route("/health", axum::routing::get(health_handler))
                .route("/api/v1/system/info", axum::routing::get(api::system::info))
                .route(
                    "/webhook/github",
                    axum::routing::post(api::webhook::handle_github_webhook),
                )
                .with_state(state),
        )
        .fallback(r#static::handle)
    } else {
        haimen_xiaozhi::add_routes(
            axum::Router::new()
                .route("/health", axum::routing::get(health_handler))
                .route("/api/v1/system/info", axum::routing::get(api::system::info)),
        )
        .fallback(r#static::handle)
    };

    let addr: SocketAddr = format!("{}:{}", config.host, config.port)
        .parse()
        .map_err(|e| format!("无效的监听地址: {}", e))?;

    tracing::info!("Web 服务器启动于 http://{addr}");

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| format!("绑定地址失败: {}", e))?;

    axum::serve(listener, app.into_make_service())
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|e| format!("服务器错误: {}", e))
}

/// 健康检查：返回 JSON
async fn health_handler() -> impl axum::response::IntoResponse {
    axum::Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "service": "haimen"
    }))
}

/// 等待 SIGINT（Ctrl+C）或 SIGTERM 用于优雅关闭
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("无法安装 Ctrl+C 处理器");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("无法安装 SIGTERM 处理器")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("收到 Ctrl+C，正在关闭..."),
        _ = terminate => tracing::info!("收到 SIGTERM，正在关闭..."),
    }
}
