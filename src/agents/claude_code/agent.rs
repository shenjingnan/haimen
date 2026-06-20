use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing;

use crate::gateway::provider::AgentProvider;

/// Claude Code Agent
///
/// 通过 `claude --print` 子进程调用 Claude Code 处理消息。
pub struct ClaudeAgent;

#[async_trait]
impl AgentProvider for ClaudeAgent {
    fn name(&self) -> &str {
        "claude-code"
    }

    async fn process(
        &self,
        message: &str,
        session_id: Option<&str>,
    ) -> Result<(String, String), String> {
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

/// 调用 claude --print 流式处理
async fn process_with_claude_stream(
    prompt: &str,
    session_id: Option<&str>,
) -> Result<(String, String), String> {
    let mut args: Vec<String> = vec![
        "--print".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
        "--include-partial-messages".to_string(),
    ];

    if let Some(sid) = session_id {
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

    let reader = BufReader::new(stdout);
    let mut lines = reader.lines();

    let mut full_response = String::new();
    let mut extracted_session_id: Option<String> = None;
    let mut system_init_parsed = false;

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
                if let Some(sid) = json.get("session_id").and_then(|v| v.as_str()) {
                    extracted_session_id = Some(sid.to_string());
                    tracing::debug!(session_id = %sid, "提取到 session_id");
                }
            }

            Some("stream_event") => {
                if let Some(event) = json.get("event") {
                    if let Some("content_block_delta") = event.get("type").and_then(|v| v.as_str())
                    {
                        if let Some(delta) = event.get("delta") {
                            if let Some("text_delta") = delta.get("type").and_then(|v| v.as_str()) {
                                if let Some(text) = delta.get("text").and_then(|v| v.as_str()) {
                                    full_response.push_str(text);
                                }
                            }
                        }
                    }
                }
                if full_response.is_empty() {
                    if let Some("content_block_delta") =
                        json.get("event_type").and_then(|v| v.as_str())
                    {
                        if let Some(delta) = json.get("delta") {
                            if let Some("text_delta") = delta.get("type").and_then(|v| v.as_str()) {
                                if let Some(text) = delta.get("text").and_then(|v| v.as_str()) {
                                    full_response.push_str(text);
                                }
                            }
                        }
                    }
                }
            }

            Some("assistant") if full_response.is_empty() => {
                if let Some(content) = json.get("content").and_then(|c| c.as_array()) {
                    for block in content {
                        if let Some("text") = block.get("type").and_then(|v| v.as_str()) {
                            if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                                full_response.push_str(text);
                            }
                        }
                    }
                }
            }

            Some("result") if extracted_session_id.is_none() => {
                if let Some(sid) = json.get("session_id").and_then(|v| v.as_str()) {
                    extracted_session_id = Some(sid.to_string());
                }
            }

            _ => {}
        }
    }

    let _ = child.wait().await;

    let final_response = full_response.trim().to_string();
    let final_session_id =
        extracted_session_id.ok_or_else(|| "无法从 claude 输出中提取 session_id".to_string())?;

    if final_response.is_empty() {
        return Err("Claude 返回为空".to_string());
    }

    Ok((final_response, final_session_id))
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
