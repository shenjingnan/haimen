pub mod agent_handle;
pub mod channel;
pub mod chat_loop;
pub mod model;
pub mod provider;
pub mod session;
pub mod webhook;

use std::sync::{Arc, RwLock};

use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::agents::registry::registry;
use crate::config::settings::load_settings;
use crate::connectors::dingtalk::channel::DingTalkChannel;
use crate::connectors::github::GitHubConnector;
use crate::gateway::agent_handle::{build_shared_agent, current_agent};
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
            let dingtalk_cfg: crate::connectors::dingtalk::config::DingTalkConfig =
                dt_cfg.clone().into();
            channels.push((
                "dingtalk".to_string(),
                Box::new(DingTalkChannel::new(dingtalk_cfg)) as Box<dyn MessageChannel>,
            ));
        }
    }

    Ok(channels)
}

/// 根据配置构建 Agent
///
/// 通过 [`crate::agents::registry::registry`] 按 `active_provider` 分发，
/// 新增 Agent 无需改动此处。
pub fn build_agent(
    config: &crate::config::settings::AppConfig,
) -> Result<Box<dyn AgentProvider>, String> {
    let agent_name = config.gateway.resolved_agent();
    registry().build(&agent_name, &config.gateway)
}

/// 根据配置和环境变量构造 xiaozhi WebSocket 响应策略
///
/// 固定文本模式（`tts.fixed_text_enabled = true`）会保留 ASR 流式管线用于 VAD 判停，
/// 但跳过 LLM 处理，直接使用预设文本进行 TTS 合成。
///
/// 需要配置 ASR 和 TTS 提供商凭证。环境变量缺失时跳过 xiaozhi 路由（不挂载）。
///
/// ASR 配置通过 Arc<RwLock> 共享，Web API 保存时同步更新此对象，实现运行时热加载。
/// Agent 使用共享句柄，Web API 切换时同步更新，实现运行时热切换。
fn build_xiaozhi_strategy(
    shared_agent: crate::gateway::agent_handle::SharedAgent,
    config: &crate::config::settings::AppConfig,
    shared_asr_config: crate::xiaozhi_asr_llm_tts::SharedAsrConfig,
    shared_tts_config: crate::xiaozhi_asr_llm_tts::SharedTtsConfig,
) -> Option<Arc<dyn haimen_xiaozhi::ResponseStrategy>> {
    // 检查 ASR 凭证（所有模式都需要 ASR 用于 VAD 判停）
    {
        let cfg = shared_asr_config.read().unwrap();
        let has_creds = match cfg.active_provider.as_str() {
            "doubao" => cfg.get_credential("api_key").is_some(),
            "xfyun" => {
                cfg.get_credential("app_id").is_some()
                    && cfg.get_credential("api_key").is_some()
                    && cfg.get_credential("api_secret").is_some()
            }
            // qwen / glm / mimo 等使用 api_key
            _ => cfg.get_credential("api_key").is_some(),
        };
        if !has_creds {
            tracing::info!(
                provider = %cfg.active_provider,
                "未配置 ASR 凭证，xiaozhi WebSocket 不启动",
            );
            return None;
        }
    }

    let work_dir = resolve_work_dir_from_config(config);
    Some(Arc::new(
        crate::xiaozhi_asr_llm_tts::AsrLlmTtsStrategy::new(
            shared_asr_config,
            shared_tts_config,
            None, // voice_override
            shared_agent,
            work_dir,
        ),
    ))
}

/// 从 AppConfig 解析工作目录
fn resolve_work_dir_from_config(config: &crate::config::settings::AppConfig) -> String {
    crate::gateway::chat_loop::resolve_work_dir(config.gateway.work_dir.clone())
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
pub async fn start_all(cli_open_browser: bool) -> Result<(), String> {
    let config = load_settings().ok().flatten().unwrap_or_default();

    // 清理过期 Agent 调用日志
    let removed = crate::agent_log::cleanup(config.agent_log.retention_days);
    if removed > 0 {
        tracing::info!(removed = removed, "启动时清理过期 Agent 调用日志");
    }

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

    // 构建共享 Agent 句柄（所有消费路径共用，支持 Web API 运行时热切换）
    let shared_agent = build_shared_agent(&config, None)?;

    // 启动 HTTP 服务器（xiaozhi + GitHub Webhook + Web 控制台）
    let http_handle = if config.http.enabled {
        let http_cancel = cancel.clone();
        let serve_config = crate::web::ServeConfig {
            host: config.http.host.clone(),
            port: config.http.port,
            auto_open: cli_open_browser,
        };

        let work_dir = resolve_work_dir_from_config(&config);

        // GitHub Webhook（可选）
        let webhook_state = config.github.clone().map(|cfg| {
            let connector = GitHubConnector::new(cfg, shared_agent.clone(), work_dir.clone());
            WebhookState {
                github: Some(Arc::new(connector)),
            }
        });

        let shared_asr_config = Arc::new(RwLock::new(config.asr.clone()));
        let shared_tts_config = Arc::new(RwLock::new(config.tts.clone()));
        let xiaozhi_strategy = build_xiaozhi_strategy(
            shared_agent.clone(),
            &config,
            shared_asr_config.clone(),
            shared_tts_config.clone(),
        );

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

        let http_shared_agent = shared_agent.clone();
        let handle = tokio::spawn(async move {
            let result = crate::web::start(
                serve_config,
                webhook_state,
                xiaozhi_strategy,
                shared_asr_config,
                shared_tts_config,
                http_shared_agent,
                http_cancel,
            )
            .await;
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

        current_agent(&shared_agent).check_available().await?;

        tracing::info!(
            "haimen 已启动 — 连接器: {:?}, HTTP: {}, Agent: {}",
            channels
                .iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<&str>>(),
            if config.http.enabled { "是" } else { "否" },
            current_agent(&shared_agent).name(),
        );

        let result = chat_loop::run_unified_gateway(
            channels,
            &shared_agent,
            &config.gateway,
            cancel.clone(),
        )
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

    let shared_agent = build_shared_agent(&config, None)?;
    current_agent(&shared_agent).check_available().await?;

    chat_loop::run_chat_loop(&*channel, &shared_agent, &config.gateway).await
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
