use async_trait::async_trait;
use futures_util::StreamExt;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing;

use crate::gateway::provider::{AgentProvider, TextStream};

/// Claude Code Agent
///
/// 通过 `claude --print` 子进程调用 Claude Code 处理消息。
/// 同时支持批处理（process）和流式处理（process_stream）。
pub struct ClaudeAgent;

#[async_trait]
impl AgentProvider for ClaudeAgent {
    fn name(&self) -> &str {
        "claude-code"
    }

    /// 批处理：等待 claude 全部输出完成后返回完整文本
    async fn process(
        &self,
        message: &str,
        session_id: Option<&str>,
    ) -> Result<(String, String), String> {
        let (mut stream, sid) = self.process_stream(message, session_id).await?;
        let mut full_text = String::new();
        while let Some(chunk) = stream.next().await {
            full_text.push_str(&chunk);
        }
        let final_text = full_text.trim().to_string();
        if final_text.is_empty() {
            return Err("Claude 返回为空".to_string());
        }
        Ok((final_text, sid))
    }

    /// 流式处理：逐块返回 claude 的文本输出，实现 Agent 输出与 TTS 合成的并行
    async fn process_stream(
        &self,
        message: &str,
        session_id: Option<&str>,
    ) -> Result<(TextStream, String), String> {
        process_with_claude_stream(message, session_id).await
    }

    async fn check_available(&self) -> Result<(), String> {
        if check_claude_available().await {
            Ok(())
        } else {
            Err("claude CLI 未安装。请执行: npm install -g @anthropic-ai/claude-code".to_string())
        }
    }
}

/// 启动 claude --print 子进程，返回文本流和 session_id
async fn process_with_claude_stream(
    prompt: &str,
    resume_session_id: Option<&str>,
) -> Result<(TextStream, String), String> {
    let mut args: Vec<String> = vec![
        "--print".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
        "--include-partial-messages".to_string(),
    ];

    if let Some(sid) = resume_session_id {
        args.push("--resume".to_string());
        args.push(sid.to_string());
    }

    args.push(prompt.to_string());

    let mut child = Command::new("claude")
        .args(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("启动 claude 失败: {}", e))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "无法获取 claude stdout".to_string())?;

    // 两个 channel：session_id（oneshot）+ 文本流（mpsc）
    let (sid_tx, sid_rx) = tokio::sync::oneshot::channel::<String>();
    let (text_tx, text_rx) = mpsc::channel::<String>(64);

    // 后台读取任务：解析 JSON 行，提取文本块发送到 channel
    tokio::spawn(async move {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();
        let mut system_init_parsed = false;
        let mut sid_tx = Some(sid_tx);

        while let Ok(Some(line)) = lines.next_line().await {
            if line.trim().is_empty() {
                continue;
            }

            let json: serde_json::Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let type_str = json.get("type").and_then(|v| v.as_str());

            match type_str {
                Some("system") if !system_init_parsed => {
                    system_init_parsed = true;
                    if let Some(tx) = sid_tx.take() {
                        if let Some(sid) = json.get("session_id").and_then(|v| v.as_str()) {
                            let _ = tx.send(sid.to_string());
                        }
                    }
                }

                Some("stream_event") => {
                    // 嵌套格式: {"event": {"type": "content_block_delta", "delta": ...}}
                    if let Some(text) = extract_text_from_event(&json) {
                        if text_tx.send(text).await.is_err() {
                            break;
                        }
                        continue; // 已通过嵌套格式提取，跳过扁平格式
                    }
                    // 扁平格式: {"event_type": "content_block_delta", "delta": ...}
                    if let Some(text) = extract_text_from_flat(&json) {
                        if text_tx.send(text).await.is_err() {
                            break;
                        }
                    }
                }

                Some("result") if system_init_parsed => {
                    // result 消息——忽略已有 session_id 的情况
                }
                Some("result") => {
                    if let Some(tx) = sid_tx.take() {
                        if let Some(sid) = json.get("session_id").and_then(|v| v.as_str()) {
                            let _ = tx.send(sid.to_string());
                        }
                    }
                }

                _ => {}
            }
        }

        // 确保子进程退出
        let _ = child.wait().await;
    });

    // 等待 session_id（来自 system 或 result 消息）
    let sid = tokio::time::timeout(std::time::Duration::from_secs(30), sid_rx)
        .await
        .map_err(|_| "等待 claude session_id 超时 (30s)".to_string())?
        .map_err(|_| "无法从 claude 输出中提取 session_id".to_string())?;

    tracing::debug!(session_id = %sid, "process_stream: session_id 已提取");

    let stream: TextStream = Box::pin(ReceiverStream::new(text_rx));
    Ok((stream, sid))
}

/// 从嵌套 stream_event 格式中提取文本 delta
///
/// ```json
/// {"type": "stream_event", "event": {"type": "content_block_delta", "delta": {"type": "text_delta", "text": "hello"}}}
/// ```
fn extract_text_from_event(json: &serde_json::Value) -> Option<String> {
    let event = json.get("event")?;
    if event.get("type")?.as_str()? != "content_block_delta" {
        return None;
    }
    let delta = event.get("delta")?;
    if delta.get("type")?.as_str()? != "text_delta" {
        return None;
    }
    delta.get("text")?.as_str().map(String::from)
}

/// 从扁平 stream_event 格式中提取文本 delta
///
/// ```json
/// {"type": "stream_event", "event_type": "content_block_delta", "delta": {"type": "text_delta", "text": "hello"}}
/// ```
fn extract_text_from_flat(json: &serde_json::Value) -> Option<String> {
    if json.get("event_type")?.as_str()? != "content_block_delta" {
        return None;
    }
    let delta = json.get("delta")?;
    if delta.get("type")?.as_str()? != "text_delta" {
        return None;
    }
    delta.get("text")?.as_str().map(String::from)
}

/// 检查 claude CLI 是否可用
async fn check_claude_available() -> bool {
    Command::new("claude")
        .arg("--version")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}
