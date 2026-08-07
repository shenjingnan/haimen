use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use futures_util::StreamExt;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::ReceiverStream;
use tracing;

use crate::config::settings::GatewayConfig;
use crate::gateway::provider::{
    AgentEventStream, AgentLogEvent, AgentOutput, AgentProvider, TextStream,
};

/// 默认 OpenClaw agent id（OpenClaw 保留 agent）
pub const DEFAULT_AGENT_ID: &str = "main";

/// 等待 openclaw 子进程输出（一次性 JSON）的硬超时
const SESSION_WAIT_SECS: u64 = 30;

/// OpenClaw CLI Agent
///
/// 通过 `openclaw agent --json` 子进程调用。
///
/// - 默认走 Gateway RPC（openclaw gateway 常驻，如 LaunchAgent 端口 18789）；
///   Gateway 缺失时 openclaw 自动降级 embedded（需 shell 有模型 keys），
///   haimen 不管理 gateway 生命周期。
/// - CLI 层非流式（结尾一次性 JSON）：`process_stream` 单块返回完整回复，
///   语音场景（xiaozhi）TTS 等全部回复生成完再合成，无 thinking/tool 事件轨迹。
/// - 会话：haimen 侧生成/持有完整 session key（`agent:<id>:haimen:<unique>`），
///   新会话生成唯一 key，resume 时原样传回 `--session-key`。
pub struct OpenClawAgent {
    /// openclaw agent id（`--agent <id>`），默认 "main"
    agent: String,
    /// 模型调用超时秒数（`--timeout`），与网关 agent_timeout_secs 对齐
    timeout_secs: u64,
}

impl OpenClawAgent {
    /// 使用指定 agent id 与超时构造
    pub fn new(agent: impl Into<String>, timeout_secs: u64) -> Self {
        Self {
            agent: agent.into(),
            timeout_secs,
        }
    }

    /// 当前 agent id（供测试断言使用）
    #[cfg(test)]
    pub(crate) fn agent(&self) -> &str {
        &self.agent
    }

    /// 当前超时秒数（供测试断言使用）
    #[cfg(test)]
    pub(crate) fn timeout_secs(&self) -> u64 {
        self.timeout_secs
    }
}

/// 从网关配置解析 openclaw agent id
///
/// 优先读取 `[gateway.providers.openclaw] agent`，缺省使用 [`DEFAULT_AGENT_ID`]。
pub fn resolve_agent(config: &GatewayConfig) -> String {
    config
        .providers
        .get("openclaw")
        .and_then(|p| p.get("agent"))
        .filter(|v| !v.is_empty())
        .cloned()
        .unwrap_or_else(|| DEFAULT_AGENT_ID.to_string())
}

#[async_trait]
impl AgentProvider for OpenClawAgent {
    fn name(&self) -> &str {
        "openclaw"
    }

    /// 批处理：等待 openclaw 全部输出完成后返回完整文本（无事件轨迹）
    async fn process(
        &self,
        message: &str,
        session_id: Option<&str>,
        work_dir: &str,
    ) -> Result<(AgentOutput, String), String> {
        let (mut stream, sid, _events_rx) =
            self.process_stream(message, session_id, work_dir).await?;
        let mut full_text = String::new();
        while let Some(chunk) = stream.next().await {
            full_text.push_str(&chunk);
        }
        let final_text = full_text.trim().to_string();
        if final_text.is_empty() {
            return Err("OpenClaw 返回为空".to_string());
        }
        Ok((
            AgentOutput {
                text: final_text,
                events: Vec::new(),
            },
            sid,
        ))
    }

    /// 流式处理：单块返回完整回复（openclaw CLI 非流式），事件流为空
    async fn process_stream(
        &self,
        message: &str,
        session_id: Option<&str>,
        work_dir: &str,
    ) -> Result<(TextStream, String, AgentEventStream), String> {
        let (stream, sid) = process_with_openclaw(
            message,
            session_id,
            work_dir,
            &self.agent,
            self.timeout_secs,
        )
        .await?;
        // openclaw CLI 不暴露 thinking/tool 事件，事件流为空（sender 立即 drop）
        let (_tx, rx) = mpsc::channel::<AgentLogEvent>(64);
        Ok((stream, sid, rx))
    }

    async fn check_available(&self) -> Result<(), String> {
        if check_openclaw_available().await {
            Ok(())
        } else {
            Err("openclaw CLI 未安装。请执行: npm install -g openclaw".to_string())
        }
    }
}

/// 生成唯一 OpenClaw session key：`agent:<id>:haimen:<nanos><seq>`
///
/// 新会话时使用——必须显式传 key，否则 openclaw 会把不指定会话的调用
/// 全部落到默认 `agent:<id>:main` 会话，导致所有新会话共享上下文。
fn generate_session_key(agent: &str) -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("agent:{}:haimen:{:020x}{:08x}", agent, nanos, seq)
}

/// 启动 `openclaw agent --json` 子进程，返回文本流与 session key
///
/// 会话策略：haimen 侧持有完整 session key。
/// - resume（session_id = Some）→ 原样作为 `--session-key`
/// - 新会话（None）→ 生成唯一 key
///
/// 返回的 new session_id 即本次实际使用的 key，下次 resume 直接传回。
async fn process_with_openclaw(
    prompt: &str,
    session_id: Option<&str>,
    work_dir: &str,
    agent: &str,
    timeout_secs: u64,
) -> Result<(TextStream, String), String> {
    // 新会话必须显式生成 key，避免落到 openclaw 默认 main 会话（共享上下文）
    let session_key = match session_id {
        Some(k) if !k.trim().is_empty() => k.trim().to_string(),
        _ => generate_session_key(agent),
    };

    let args = build_openclaw_args(prompt, &session_key, agent, timeout_secs);

    tracing::debug!(args = ?args, "启动 openclaw 子进程");

    // Windows 上 openclaw 是 npm 安装的 .cmd shim，经 build_command 解析包装
    let mut child = Command::from(haimen_core::process::build_command("openclaw", &args))
        .current_dir(work_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("启动 openclaw 失败: {}", e))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "无法获取 openclaw stdout".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "无法获取 openclaw stderr".to_string())?;

    // 结果信号：stdout 一次性 JSON，解析成功后送 Ok，失败送 Err(具体文案)
    let (result_tx, result_rx) = oneshot::channel::<Result<(), String>>();
    let (text_tx, text_rx) = mpsc::channel::<String>(64);

    // 后台读 stderr（openclaw 将进度/错误日志导向 stderr）
    tokio::spawn(async move {
        let reader = BufReader::new(stderr);
        let mut lines = reader.lines();
        let mut stderr_buf = Vec::new();
        while let Ok(Some(line)) = lines.next_line().await {
            if !line.trim().is_empty() {
                stderr_buf.push(line);
            }
        }
        if !stderr_buf.is_empty() {
            tracing::warn!(stderr_lines = stderr_buf.len(), "openclaw stderr 输出：");
            for line in &stderr_buf {
                tracing::warn!(stderr = %line, "openclaw stderr");
            }
        }
    });

    // 后台读 stdout：非流式，整段收集后一次性解析 JSON
    tokio::spawn(async move {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();
        let mut raw = String::new();
        while let Ok(Some(line)) = lines.next_line().await {
            raw.push_str(&line);
            raw.push('\n');
        }

        match parse_openclaw_output(&raw) {
            Ok(text) => {
                // 先送文本再发信号，确保消费者拿到 result 时文本已在通道中
                let _ = text_tx.send(text).await;
                let _ = result_tx.send(Ok(()));
            }
            Err(e) => {
                tracing::error!(error = %e, "openclaw 处理失败");
                let _ = result_tx.send(Err(e));
            }
        }

        // 确保子进程退出
        let _ = child.wait().await;
    });

    // 等待结果信号；失败透传具体错误文案
    tokio::time::timeout(std::time::Duration::from_secs(SESSION_WAIT_SECS), result_rx)
        .await
        .map_err(|_| "等待 openclaw 输出超时 (30s)".to_string())?
        .map_err(|_| "无法从 openclaw 输出中提取回复".to_string())??;

    let stream: TextStream = Box::pin(ReceiverStream::new(text_rx));
    Ok((stream, session_key))
}

/// 构建 `openclaw agent` 参数
///
/// 统一使用 `--agent <id>` + `--session-key <key>`：
/// - session key 由 haimen 侧生成/持有（`agent:<id>:haimen:<unique>`）
/// - 必须显式传 key：否则 openclaw 会把不指定会话的调用落到默认
///   `agent:<id>:main` 会话，所有新会话共享上下文
fn build_openclaw_args(
    prompt: &str,
    session_key: &str,
    agent: &str,
    timeout_secs: u64,
) -> Vec<String> {
    vec![
        "agent".to_string(),
        "--json".to_string(),
        "--timeout".to_string(),
        timeout_secs.to_string(),
        "--agent".to_string(),
        agent.to_string(),
        "--session-key".to_string(),
        session_key.to_string(),
        "-m".to_string(),
        prompt.to_string(),
    ]
}

/// 解析 `openclaw agent --json` 的完整 stdout，提取最终回复文本
///
/// 期望 schema（v2026.7.1 实测）：
/// ```json
/// { "status": "ok", "result": { "payloads": [ { "text": "..." } ] } }
/// ```
/// - 文本取 `result.payloads[].text`，多段以 `\n` 连接，跳过空段
/// - 文本为空：优先透传顶层 `error` 字段文案，否则报 "OpenClaw 返回为空"
fn parse_openclaw_response(json: &serde_json::Value) -> Result<String, String> {
    let mut parts = Vec::new();
    if let Some(payloads) = json
        .get("result")
        .and_then(|r| r.get("payloads"))
        .and_then(|p| p.as_array())
    {
        for payload in payloads {
            if let Some(t) = payload.get("text").and_then(|v| v.as_str()) {
                if !t.is_empty() {
                    parts.push(t.to_string());
                }
            }
        }
    }
    let text = parts.join("\n");
    if text.trim().is_empty() {
        if let Some(e) = json
            .get("error")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            return Err(format!("OpenClaw 返回错误: {}", e));
        }
        return Err("OpenClaw 返回为空".to_string());
    }
    Ok(text)
}

/// 解析 openclaw stdout：整段收集后解析 JSON，失败时截取 `{...}` 子串再试
fn parse_openclaw_output(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("openclaw 未产生输出".to_string());
    }
    let json = serde_json::from_str::<serde_json::Value>(trimmed)
        .ok()
        .or_else(|| extract_json_span(trimmed).and_then(|s| serde_json::from_str(s).ok()))
        .ok_or_else(|| format!("openclaw 输出非 JSON: {}", truncate(trimmed, 300)))?;
    parse_openclaw_response(&json)
}

/// 截取字符串中首个 `{` 到末个 `}` 的 JSON 子串（stdout 混入进度日志时兜底）
fn extract_json_span(s: &str) -> Option<&str> {
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    if end <= start {
        return None;
    }
    Some(&s[start..=end])
}

/// 截断长字符串用于报错文案
fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max).collect();
        t.push('…');
        t
    }
}

/// 检查 openclaw CLI 是否可用
async fn check_openclaw_available() -> bool {
    Command::from(haimen_core::process::build_command(
        "openclaw",
        &["--version".to_string()],
    ))
    .output()
    .await
    .map(|o| o.status.success())
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> GatewayConfig {
        GatewayConfig::default()
    }

    #[test]
    fn test_openclaw_agent_name() {
        let agent = OpenClawAgent::new(DEFAULT_AGENT_ID, 300);
        assert_eq!(agent.name(), "openclaw");
        assert_eq!(agent.agent(), DEFAULT_AGENT_ID);
        assert_eq!(agent.timeout_secs(), 300);
    }

    #[test]
    fn test_build_args_new_session_explicit_key() {
        // 新会话也必须是显式 session-key，避免落到默认 main 会话
        let args = build_openclaw_args("hello", "agent:main:haimen:abc", "main", 300);
        assert_eq!(
            args,
            vec![
                "agent".to_string(),
                "--json".to_string(),
                "--timeout".to_string(),
                "300".to_string(),
                "--agent".to_string(),
                "main".to_string(),
                "--session-key".to_string(),
                "agent:main:haimen:abc".to_string(),
                "-m".to_string(),
                "hello".to_string(),
            ]
        );
    }

    #[test]
    fn test_build_args_custom_agent() {
        let args = build_openclaw_args("hi", "agent:ops:haimen:xyz", "ops", 600);
        assert!(args.contains(&"--agent".to_string()));
        assert!(args.contains(&"ops".to_string()));
        assert!(args.contains(&"--timeout".to_string()));
        assert!(args.contains(&"600".to_string()));
        assert!(args.contains(&"--session-key".to_string()));
        assert!(args.contains(&"agent:ops:haimen:xyz".to_string()));
    }

    #[test]
    fn test_resolve_agent_default() {
        // 未配置时回退到默认 agent
        assert_eq!(resolve_agent(&test_config()), DEFAULT_AGENT_ID);
    }

    #[test]
    fn test_resolve_agent_custom() {
        // 配置 [gateway.providers.openclaw] agent 后应被读取
        let mut config = test_config();
        let mut providers = std::collections::HashMap::new();
        let mut params = std::collections::HashMap::new();
        params.insert("agent".to_string(), "ops".to_string());
        providers.insert("openclaw".to_string(), params);
        config.providers = providers;
        assert_eq!(resolve_agent(&config), "ops");
    }

    #[test]
    fn test_resolve_agent_ignores_other_providers() {
        // 其他 provider 的 agent 配置不影响 openclaw
        let mut config = test_config();
        let mut providers = std::collections::HashMap::new();
        let mut params = std::collections::HashMap::new();
        params.insert("agent".to_string(), "whatever".to_string());
        providers.insert("codex".to_string(), params);
        config.providers = providers;
        assert_eq!(resolve_agent(&config), DEFAULT_AGENT_ID);
    }

    #[test]
    fn test_parse_openclaw_response_success() {
        // 实际 v2026.7.1 响应中与文本相关的结构
        let json = serde_json::json!({
            "runId": "run-1",
            "status": "ok",
            "result": {
                "payloads": [ { "text": "pong", "mediaUrl": null } ]
            }
        });
        assert_eq!(parse_openclaw_response(&json).as_deref(), Ok("pong"));
    }

    #[test]
    fn test_parse_openclaw_response_multiple_payloads() {
        // 多段 payload 以换行连接
        let json = serde_json::json!({
            "status": "ok",
            "result": {
                "payloads": [
                    { "text": "第一段" },
                    { "text": "第二段" }
                ]
            }
        });
        assert_eq!(
            parse_openclaw_response(&json).as_deref(),
            Ok("第一段\n第二段")
        );
    }

    #[test]
    fn test_parse_openclaw_response_media_only_payload() {
        // 仅含 mediaUrl 的段不计入文本
        let json = serde_json::json!({
            "status": "ok",
            "result": {
                "payloads": [
                    { "text": "", "mediaUrl": "https://example.com/a.png" },
                    { "text": "done", "mediaUrl": null }
                ]
            }
        });
        assert_eq!(parse_openclaw_response(&json).as_deref(), Ok("done"));
    }

    #[test]
    fn test_parse_openclaw_response_empty() {
        // 无 payloads 或空文本 → 报 "OpenClaw 返回为空"
        let json = serde_json::json!({ "status": "ok", "result": { "payloads": [] } });
        assert_eq!(
            parse_openclaw_response(&json).unwrap_err(),
            "OpenClaw 返回为空"
        );
    }

    #[test]
    fn test_parse_openclaw_response_error_field() {
        // 顶层 error 字段文案透传
        let json = serde_json::json!({ "status": "error", "error": "model unavailable" });
        assert_eq!(
            parse_openclaw_response(&json).unwrap_err(),
            "OpenClaw 返回错误: model unavailable"
        );
    }

    #[test]
    fn test_parse_openclaw_output_not_json() {
        // 纯文本输出 → 报错并包含原文
        let err = parse_openclaw_output("some plain text").unwrap_err();
        assert!(err.contains("openclaw 输出非 JSON"));
    }

    #[test]
    fn test_parse_openclaw_output_extract_span() {
        // stdout 混入日志时截取 {..} 子串再解析
        let raw =
            "progress...\n{\"status\":\"ok\",\"result\":{\"payloads\":[{\"text\":\"hi\"}]}}\n";
        assert_eq!(parse_openclaw_output(raw).as_deref(), Ok("hi"));
    }

    #[test]
    fn test_extract_json_span() {
        let raw = "prefix { \"a\": 1 } suffix";
        assert_eq!(extract_json_span(raw), Some("{ \"a\": 1 }"));
    }

    #[test]
    fn test_extract_json_span_no_braces() {
        assert_eq!(extract_json_span("no braces here"), None);
    }

    #[test]
    fn test_generate_session_key_unique_and_formatted() {
        let key1 = generate_session_key("main");
        let key2 = generate_session_key("main");
        assert_ne!(key1, key2, "两次生成的 key 不应相同");
        assert!(
            key1.starts_with("agent:main:haimen:"),
            "key 前缀错误: {}",
            key1
        );
        assert!(key2.starts_with("agent:main:haimen:"));
    }
}
