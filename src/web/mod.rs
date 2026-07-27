pub mod api;
pub mod r#static;

use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use tokio_util::sync::CancellationToken;

use crate::config::settings::{AsrConfig, TtsConfig};
use crate::gateway::webhook::WebhookState;

/// 服务器配置
pub struct ServeConfig {
    pub host: String,
    pub port: u16,
    /// 启动成功后自动打开浏览器
    pub auto_open: bool,
}

/// 启动 HTTP 服务器
///
/// - `config`: 服务器地址配置
/// - `webhook_state`: 可选的 Webhook 处理器（GitHub Webhook）
/// - `xiaozhi_strategy`: 可选的 xiaozhi WebSocket 响应策略，为 None 时不挂载 xiaozhi 路由
/// - `asr_config`: 共享的 ASR 配置（Arc<RwLock>），Web API 保存时同步更新，实现运行时热加载
/// - `tts_config`: 共享的 TTS 配置（Arc<RwLock>），Web API 保存时同步更新，实现运行时热加载
/// - `cancel`: 共享取消令牌，收到取消信号时触发优雅关闭
pub async fn start(
    config: ServeConfig,
    webhook_state: Option<WebhookState>,
    xiaozhi_strategy: Option<Arc<dyn haimen_xiaozhi::ResponseStrategy>>,
    asr_config: Arc<RwLock<AsrConfig>>,
    tts_config: Arc<RwLock<TtsConfig>>,
    cancel: CancellationToken,
) -> Result<(), String> {
    use haimen_xiaozhi;

    // 构建路由
    // ASR 路由使用 asr_config 作为 axum State（独立于 TTS 配置）
    let asr_routes = axum::Router::new()
        .route(
            "/api/v1/settings/asr",
            axum::routing::get(api::voice_settings::get_asr_settings),
        )
        .route(
            "/api/v1/settings/asr",
            axum::routing::put(api::voice_settings::update_asr_settings),
        )
        .route(
            "/api/v1/settings/asr/verify",
            axum::routing::post(api::voice_settings::verify_asr_credentials),
        )
        .with_state(asr_config);

    // TTS 路由使用 tts_config 作为 axum State
    let tts_routes = axum::Router::new()
        .route(
            "/api/v1/settings/tts",
            axum::routing::get(api::voice_settings::get_tts_settings),
        )
        .route(
            "/api/v1/settings/tts",
            axum::routing::put(api::voice_settings::update_tts_settings),
        )
        .route(
            "/api/v1/settings/tts/voices",
            axum::routing::get(api::voice_settings::list_tts_voices),
        )
        .route(
            "/api/v1/settings/tts/verify",
            axum::routing::post(api::voice_settings::verify_tts_credentials),
        )
        .with_state(tts_config);

    let agent_routes = axum::Router::new()
        .route(
            "/api/v1/settings/agent",
            axum::routing::get(api::agent_settings::get_agent_settings),
        )
        .route(
            "/api/v1/settings/agent",
            axum::routing::put(api::agent_settings::update_agent_settings),
        )
        .route(
            "/api/v1/settings/agent/verify",
            axum::routing::post(api::agent_settings::verify_agent_credentials),
        );

    let app = if let Some(state) = webhook_state {
        let mut r = axum::Router::new()
            .route("/health", axum::routing::get(health_handler))
            .route("/api/v1/system/info", axum::routing::get(api::system::info))
            .route(
                "/webhook/github",
                axum::routing::post(api::webhook::handle_github_webhook),
            )
            .with_state(state);
        if let Some(strategy) = xiaozhi_strategy {
            r = haimen_xiaozhi::add_routes(r, strategy);
        }
        r = r.merge(asr_routes);
        r = r.merge(tts_routes);
        r = r.merge(agent_routes);
        r.fallback(r#static::handle)
    } else {
        let mut r = axum::Router::new()
            .route("/health", axum::routing::get(health_handler))
            .route("/api/v1/system/info", axum::routing::get(api::system::info));
        if let Some(strategy) = xiaozhi_strategy {
            r = haimen_xiaozhi::add_routes(r, strategy);
        }
        r = r.merge(asr_routes);
        r = r.merge(tts_routes);
        r = r.merge(agent_routes);
        r.fallback(r#static::handle)
    };

    let addr: SocketAddr = format!("{}:{}", config.host, config.port)
        .parse()
        .map_err(|e| format!("无效的监听地址: {}", e))?;

    tracing::info!("Web 服务器启动于 http://{addr}");

    // 打印 Web UI 地址（始终可见，不受日志级别影响）
    let listen_all = config.host == "0.0.0.0" || config.host == "::";
    if listen_all {
        // 监听所有接口时，同时打印 localhost 和真实 LAN IP
        println!("🌐 Web UI: http://127.0.0.1:{}", config.port);
        if let Ok(ip) = local_ip_address::local_ip() {
            println!("🌐 Web UI (LAN): http://{}:{}", ip, config.port);
        }
    } else {
        println!("🌐 Web UI: http://{}:{}", config.host, config.port);
    }

    // 自动打开浏览器（仅在用户要求时）
    if config.auto_open {
        open_browser(&config.host, config.port);
    }

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| format!("绑定地址失败: {}", e))?;

    // 优雅关闭：优先等待共享取消令牌，同时也响应 SIGINT/SIGTERM
    let graceful = graceful_shutdown(cancel);

    axum::serve(listener, app.into_make_service())
        .with_graceful_shutdown(graceful)
        .await
        .map_err(|e| format!("服务器错误: {}", e))
}

/// 组合信号 + CancellationToken 的优雅关闭 Future
async fn graceful_shutdown(cancel: CancellationToken) {
    tokio::select! {
        _ = shutdown_signal() => {
            tracing::info!("收到关闭信号，HTTP 服务器正在关闭...");
        }
        _ = cancel.cancelled() => {
            tracing::info!("网关已停止，HTTP 服务器正在关闭...");
        }
    }
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

/// 自动打开浏览器到 Web 控制台
///
/// - `0.0.0.0` 会自动转为 `127.0.0.1`（浏览器无法访问 `0.0.0.0`）
/// - 打开失败只记录 warning，不阻塞启动流程
fn open_browser(host: &str, port: u16) {
    let display_host = if host == "0.0.0.0" { "127.0.0.1" } else { host };
    let url = format!("http://{}:{}", display_host, port);
    match webbrowser::open(&url) {
        Ok(()) => tracing::info!(url = %url, "已自动打开浏览器"),
        Err(e) => tracing::warn!(url = %url, error = %e, "自动打开浏览器失败"),
    }
}
