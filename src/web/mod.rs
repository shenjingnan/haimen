use std::net::SocketAddr;

/// 服务器配置
pub struct ServeConfig {
    pub host: String,
    pub port: u16,
}

/// 启动 HTTP 服务器
pub async fn start(config: ServeConfig) -> Result<(), String> {
    let app = axum::Router::new()
        .route("/", axum::routing::get(root_handler))
        .route("/health", axum::routing::get(health_handler));

    let addr: SocketAddr = format!("{}:{}", config.host, config.port)
        .parse()
        .map_err(|e| format!("无效的监听地址: {}", e))?;

    tracing::info!("Web 服务器启动于 http://{addr}");

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| format!("绑定地址失败: {}", e))?;

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|e| format!("服务器错误: {}", e))
}

/// 主页：返回 HTML 页面
async fn root_handler() -> axum::response::Html<String> {
    axum::response::Html(format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>海门 AI 网关</title>
    <style>
        body {{ font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; max-width: 640px; margin: 80px auto; padding: 0 20px; text-align: center; color: #333; }}
        h1 {{ font-size: 2em; margin-bottom: 8px; }}
        .version {{ color: #888; font-size: 0.9em; margin-bottom: 40px; }}
        .status {{ background: #f5f5f5; border-radius: 8px; padding: 20px; }}
        .status a {{ color: #0066cc; text-decoration: none; }}
        .status a:hover {{ text-decoration: underline; }}
    </style>
</head>
<body>
    <h1>海门 AI 网关</h1>
    <p class="version">版本 {version}</p>
    <div class="status">
        <p>服务器运行正常</p>
        <p><a href="/health">健康检查 /health</a></p>
    </div>
</body>
</html>"#,
        version = env!("CARGO_PKG_VERSION")
    ))
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
