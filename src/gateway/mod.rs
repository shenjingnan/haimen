pub mod channel;
pub mod chat_loop;
pub mod model;
pub mod provider;
pub mod session;
pub mod webhook;

use std::sync::Arc;

use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::agents::claude_code::agent::ClaudeAgent;
use crate::config::settings::load_settings;
use crate::connectors::dingtalk::DingTalkChannel;
use crate::connectors::github::GitHubConnector;
use crate::gateway::channel::MessageChannel;
use crate::gateway::provider::AgentProvider;
use crate::gateway::webhook::WebhookState;
use haimen_lark::LarkChannel;

/// 构建后的连接器列表类型
type ConnectorVec = Vec<(String, Box<dyn MessageChannel>)>;

/// 根据配置构建所有启用的连接器
pub fn build_connectors(
    config: &crate::config::settings::AppConfig,
) -> Result<ConnectorVec, String> {
    let mut channels: ConnectorVec = Vec::new();

    // Lark
    if let Some(lark_cfg) = &config.connectors.lark {
        if lark_cfg.enabled {
            channels.push((
                "lark".to_string(),
                Box::new(LarkChannel::new(&lark_cfg.lark_cli_path)) as Box<dyn MessageChannel>,
            ));
        }
    }

    // DingTalk
    if let Some(dt_cfg) = &config.connectors.dingtalk {
        if dt_cfg.enabled {
            let dingtalk_cfg: haimen_dingtalk::DingTalkConfig = dt_cfg.clone().into();
            channels.push((
                "dingtalk".to_string(),
                Box::new(DingTalkChannel::new(dingtalk_cfg)) as Box<dyn MessageChannel>,
            ));
        }
    }

    Ok(channels)
}

/// 根据配置构建 Agent
pub fn build_agent(
    config: &crate::config::settings::AppConfig,
) -> Result<Box<dyn AgentProvider>, String> {
    let agent_name = config.gateway.agent.as_deref().unwrap_or("claude-code");

    match agent_name {
        "claude-code" => Ok(Box::new(ClaudeAgent)),
        other => Err(format!("不支持的 AI Agent: {}", other)),
    }
}

/// 根据配置和环境变量构造 xiaozhi WebSocket 响应策略
///
/// 默认使用 ASR-LLM-TTS 模式（语音识别 → Claude Code 处理 → 语音合成）。
/// 需要设置 `DOUBAO_APP_KEY` 和 `DOUBAO_ACCESS_TOKEN` 环境变量。
/// 环境变量缺失时跳过 xiaozhi 路由（不挂载）。
fn build_xiaozhi_strategy() -> Option<Arc<dyn haimen_xiaozhi::ResponseStrategy>> {
    let (app_key, access_token) = match (
        std::env::var("DOUBAO_APP_KEY"),
        std::env::var("DOUBAO_ACCESS_TOKEN"),
    ) {
        (Ok(key), Ok(token)) => (key, token),
        _ => {
            tracing::info!("未设置 DOUBAO_APP_KEY / DOUBAO_ACCESS_TOKEN，xiaozhi WebSocket 不启动");
            return None;
        }
    };

    let llm_agent: Arc<dyn AgentProvider> = Arc::new(ClaudeAgent);
    Some(Arc::new(
        crate::xiaozhi_asr_llm_tts::AsrLlmTtsStrategy::new(
            app_key,
            access_token,
            None, // 默认音色
            llm_agent,
        ),
    ))
}

/// 统一入口：启动所有启用的连接器 + Agent + HTTP 服务器
///
/// 启动后一切就绪：
/// - IM 连接器（飞书/Lark、钉钉等）监听消息
/// - HTTP 服务器提供 Web 控制台 + xiaozhi WebSocket + GitHub Webhook
/// - AI Agent 就绪等待处理
///
/// 流程：
/// 1. 构建连接器和 Agent
/// 2. 各连接器健康检查（并行，失败的跳过）
/// 3. 创建 CancellationToken + 启动信号监听
/// 4. 启动 HTTP 服务器（后台任务）
/// 5. 运行多连接器事件循环
pub async fn start_all(cli_no_browser: bool) -> Result<(), String> {
    let config = load_settings().ok().flatten().unwrap_or_default();

    let all_connectors = build_connectors(&config)?;
    let has_connectors = !all_connectors.is_empty();

    // 即使没有连接器，只要 HTTP 服务器启用就继续
    if !has_connectors && !config.http.enabled {
        tracing::info!("没有启用的连接器，也没有启用 HTTP 服务器");
        return Ok(());
    }

    // 创建 CancellationToken 并启动信号监听
    let cancel = CancellationToken::new();
    let signal_cancel = cancel.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        tracing::info!("收到关闭信号，正在停止所有服务...");
        signal_cancel.cancel();
    });

    // 启动 HTTP 服务器（xiaozhi + GitHub Webhook + Web 控制台）
    let http_handle = if config.http.enabled {
        let http_cancel = cancel.clone();
        let serve_config = crate::web::ServeConfig {
            host: config.http.host.clone(),
            port: config.http.port,
            auto_open: config.http.auto_open_browser && !cli_no_browser,
        };

        // GitHub Webhook（可选）
        let webhook_state = config.github.clone().map(|cfg| {
            let gh_agent: Arc<dyn AgentProvider> = Arc::new(ClaudeAgent);
            let connector = GitHubConnector::new(cfg, gh_agent);
            WebhookState {
                github: Some(Arc::new(connector)),
            }
        });

        let xiaozhi_strategy = build_xiaozhi_strategy();

        tracing::info!(
            "HTTP 服务器启动于 {}:{}{}",
            serve_config.host,
            serve_config.port,
            if xiaozhi_strategy.is_some() {
                "（含 xiaozhi 语音通道）"
            } else {
                ""
            }
        );

        let handle = tokio::spawn(async move {
            let result =
                crate::web::start(serve_config, webhook_state, xiaozhi_strategy, http_cancel).await;
            if let Err(ref e) = result {
                tracing::error!(error = %e, "HTTP 服务器退出");
            }
            result
        });

        Some(handle)
    } else {
        None
    };

    // 运行网关（仅当有连接器时）
    if has_connectors {
        let agent = build_agent(&config)?;

        // 并行健康检查，收集健康的连接器名
        let healthy: Vec<String> =
            futures_util::future::join_all(all_connectors.iter().map(|(name, ch)| {
                let name = name.clone();
                async move {
                    match ch.health_check().await {
                        Ok(_) => {
                            tracing::info!(connector = %name, "健康检查通过");
                            Some(name)
                        }
                        Err(e) => {
                            tracing::warn!(connector = %name, error = %e, "健康检查失败，跳过");
                            None
                        }
                    }
                }
            }))
            .await
            .into_iter()
            .flatten()
            .collect();

        if healthy.is_empty() {
            tracing::warn!("所有连接器健康检查均失败，无法启动网关");
            // HTTP 服务器仍在运行，等待信号
            cancel.cancelled().await;
            if let Some(handle) = http_handle {
                let _ = handle.await;
            }
            return Ok(());
        }

        // 对健康的连接器构建通道实例
        let config = load_settings().ok().flatten().unwrap_or_default();

        let mut channels: ConnectorVec = Vec::new();
        for name in &healthy {
            let ch = match name.as_str() {
                "lark" => {
                    let cfg = config
                        .connectors
                        .lark
                        .as_ref()
                        .ok_or_else(|| "Lark 配置不存在".to_string())?;
                    Box::new(LarkChannel::new(&cfg.lark_cli_path)) as Box<dyn MessageChannel>
                }
                "dingtalk" => {
                    let cfg = config
                        .connectors
                        .dingtalk
                        .clone()
                        .ok_or_else(|| "DingTalk 配置不存在".to_string())?;
                    Box::new(DingTalkChannel::new(cfg.into())) as Box<dyn MessageChannel>
                }
                other => return Err(format!("不支持的连接器: {}", other)),
            };
            channels.push((name.clone(), ch));
        }

        agent.check_available().await?;

        tracing::info!(
            "haimen 已启动 — 连接器: {:?}, HTTP: {}, Agent: {}",
            channels
                .iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<&str>>(),
            if config.http.enabled { "是" } else { "否" },
            agent.name(),
        );

        let result =
            chat_loop::run_unified_gateway(channels, &*agent, &config.gateway, cancel.clone())
                .await;

        // 网关已停止，触发 HTTP 服务器关闭
        cancel.cancel();

        if let Some(handle) = http_handle {
            let _ = handle.await;
        }

        tracing::info!("所有服务已停止");
        result
    } else {
        // 没有连接器，仅 HTTP 服务器运行，等待信号
        tracing::info!("haimen 已启动 — HTTP 服务器运行中（xiaozhi + Web 控制台）");
        cancel.cancelled().await;

        if let Some(handle) = http_handle {
            let _ = handle.await;
        }

        Ok(())
    }
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

/// 启动网关监听（单连接器模式，取第一个启用的连接器）
pub async fn listen() -> Result<(), String> {
    let config = load_settings().ok().flatten().unwrap_or_default();

    let mut channels = build_connectors(&config)?;
    if channels.is_empty() {
        return Err("没有启用的连接器".to_string());
    }

    let (name, channel) = channels.remove(0);
    tracing::info!(connector = %name, "单连接器模式");

    let agent = build_agent(&config)?;
    agent.check_available().await?;

    chat_loop::run_chat_loop(&*channel, &*agent, &config.gateway).await
}

/// 启动网关监听（Echo 模式，取第一个启用的连接器）
pub async fn listen_echo() -> Result<(), String> {
    let config = load_settings().ok().flatten().unwrap_or_default();

    let mut channels = build_connectors(&config)?;
    if channels.is_empty() {
        return Err("没有启用的连接器".to_string());
    }

    let (_, channel) = channels.remove(0);
    chat_loop::run_echo_loop(&*channel).await
}

/// Echo 模式：启动所有启用的连接器，直接 echo 不经过 Agent
pub async fn start_echo() -> Result<(), String> {
    let config = load_settings().ok().flatten().unwrap_or_default();

    let connectors = build_connectors(&config)?;
    if connectors.is_empty() {
        tracing::info!("没有启用的连接器");
        return Ok(());
    }

    let mut streams = Vec::new();
    for (name, ch) in &connectors {
        match ch.listen().await {
            Ok(stream) => {
                let cn = name.clone();
                let tagged = stream.map(move |msg| (cn.clone(), msg));
                streams.push(tagged);
            }
            Err(e) => {
                tracing::warn!(connector = %name, error = %e, "listen 失败，跳过");
            }
        }
    }

    if streams.is_empty() {
        tracing::warn!("没有连接器成功启动");
        return Ok(());
    }

    let mut merged = futures_util::stream::select_all(streams);
    while let Some((connector_name, message)) = merged.next().await {
        if let Some((_, ch)) = connectors.iter().find(|(n, _)| n == &connector_name) {
            let echo = format!("🔄 Echo: {}", message.content);
            let _ = ch.send(&message.conversation_id, &echo).await;
        }
    }

    Err("所有消息流意外结束".to_string())
}
