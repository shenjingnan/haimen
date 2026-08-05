use async_trait::async_trait;
use futures_util::StreamExt;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing;

use crate::gateway::provider::{
    AgentEventStream, AgentLogEvent, AgentOutput, AgentProvider, TextStream,
};

/// Codex CLI Agent
///
/// 通过 `codex exec --json` 子进程调用 Codex CLI 处理消息。
/// 同时支持批处理（process）和流式处理（process_stream）。
///
/// Codex CLI JSONL 输出格式：
/// - `thread.started` — 会话开始，含 `thread_id`
/// - `turn.started` — turn 开始
/// - `item.completed` with `item_type: "assistant_message"` — 回复文本
/// - `item.completed` with `item_type: "reasoning"` — 推理过程（跳过）
/// - `turn.completed` — turn 结束
pub struct CodexAgent;

#[async_trait]
impl AgentProvider for CodexAgent {
    fn name(&self) -> &str {
        "codex"
    }

    /// 批处理：等待 codex 全部输出完成后返回完整文本
    /// （codex 的 reasoning/tool 轨迹捕获留作后续，events 为空）
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
            return Err("Codex 返回为空".to_string());
        }
        Ok((
            AgentOutput {
                text: final_text,
                events: Vec::new(),
            },
            sid,
        ))
    }

    /// 流式处理：逐块返回 codex 的文本输出
    async fn process_stream(
        &self,
        message: &str,
        session_id: Option<&str>,
        work_dir: &str,
    ) -> Result<(TextStream, String, AgentEventStream), String> {
        let (stream, sid) = process_with_codex_stream(message, session_id, work_dir).await?;
        // codex 的 reasoning/tool 轨迹捕获留作后续，事件流为空（sender 立即 drop）
        let (_tx, rx) = tokio::sync::mpsc::channel::<AgentLogEvent>(64);
        Ok((stream, sid, rx))
    }

    async fn check_available(&self) -> Result<(), String> {
        if check_codex_available().await {
            Ok(())
        } else {
            Err("codex CLI 未安装。请执行: npm install -g @openai/codex".to_string())
        }
    }
}

/// 启动 codex exec --json 子进程，返回文本流和 session_id
async fn process_with_codex_stream(
    prompt: &str,
    resume_session_id: Option<&str>,
    work_dir: &str,
) -> Result<(TextStream, String), String> {
    let mut args: Vec<String> = vec![];

    if let Some(sid) = resume_session_id {
        // 恢复现有会话：codex exec resume <thread_id> "prompt"
        args.push("exec".to_string());
        args.push("resume".to_string());
        args.push(sid.to_string());
    } else {
        // 新会话：codex exec --json "prompt"
        args.push("exec".to_string());
        args.push("--json".to_string());
    }

    args.push(prompt.to_string());

    tracing::debug!(args = ?args, "启动 codex 子进程");

    // Windows 上 codex 是 npm 安装的 .cmd shim，需经 build_command 解析包装
    let mut child = Command::from(haimen_core::process::build_command("codex", &args))
        .current_dir(work_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("启动 codex 失败: {}", e))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "无法获取 codex stdout".to_string())?;

    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "无法获取 codex stderr".to_string())?;

    // 两个 channel：session_id（oneshot）+ 文本流（mpsc）
    let (sid_tx, sid_rx) = tokio::sync::oneshot::channel::<String>();
    let (text_tx, text_rx) = mpsc::channel::<String>(64);

    // 后台读取 stderr（记录错误信息）
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
            tracing::warn!(stderr_lines = stderr_buf.len(), "codex stderr 输出：");
            for line in &stderr_buf {
                tracing::warn!(stderr = %line, "codex stderr");
            }
        }
    });

    // 后台读取任务：解析 JSONL 行，提取文本块发送到 channel
    tokio::spawn(async move {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();
        let mut sid_tx = Some(sid_tx);
        let mut line_count = 0u64;
        let mut event_counts: std::collections::HashMap<String, u64> =
            std::collections::HashMap::new();
        let mut found_assistant = false;

        while let Ok(Some(line)) = lines.next_line().await {
            line_count += 1;

            if line.trim().is_empty() {
                continue;
            }

            // 记录 raw JSONL（最多前 10 行）
            if line_count <= 10 {
                tracing::debug!(
                    line_num = line_count,
                    raw = %line,
                    "codex stdout JSONL"
                );
            }

            let json: serde_json::Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(
                        line_num = line_count,
                        error = %e,
                        raw = %line,
                        "codex JSONL 解析失败"
                    );
                    continue;
                }
            };

            let type_str = json.get("type").and_then(|v| v.as_str()).map(String::from);

            // 统计事件类型
            if let Some(ref t) = type_str {
                *event_counts.entry(t.clone()).or_insert(0) += 1;
            }

            match type_str.as_deref() {
                // 会话开始：提取 thread_id 作为 session_id
                Some("thread.started") => {
                    if let Some(tx) = sid_tx.take() {
                        if let Some(tid) = json.get("thread_id").and_then(|v| v.as_str()) {
                            tracing::debug!(thread_id = %tid, "codex thread 已创建");
                            let _ = tx.send(tid.to_string());
                        }
                    }
                }

                // item 完成
                Some("item.completed") => {
                    // Codex 实际输出格式：
                    // {"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"..."}}
                    // 或 {"type":"item.completed","item":{"id":"item_0","type":"reasoning","text":"..."}}
                    //
                    // type 字段可能在顶层（旧版）或在 item 对象内（新版）
                    let item_type = json
                        .get("item_type")
                        .and_then(|v| v.as_str())
                        .or_else(|| {
                            // 新版格式：item.type
                            json.get("item")
                                .and_then(|v| v.get("type"))
                                .and_then(|v| v.as_str())
                        })
                        .or_else(|| {
                            // 兼容：item.item_type
                            json.get("item")
                                .and_then(|v| v.get("item_type"))
                                .and_then(|v| v.as_str())
                        });
                    match item_type {
                        Some("agent_message" | "assistant_message") => {
                            found_assistant = true;
                            tracing::debug!("codex reply 事件到达 (type={})", item_type.unwrap());
                            if let Some(text) = extract_assistant_message_text(&json) {
                                tracing::debug!(text_len = text.len(), "提取到回复文本");
                                if text_tx.send(text).await.is_err() {
                                    break;
                                }
                            } else {
                                tracing::warn!(
                                    raw = %line,
                                    "codex reply 文本提取失败"
                                );
                            }
                        }
                        Some("reasoning") => {
                            // 跳过推理过程
                        }
                        _ => {
                            if line_count <= 10 {
                                tracing::debug!(
                                    item_type = ?item_type,
                                    "codex item.completed（跳过）"
                                );
                            }
                        }
                    }
                }

                // turn 完成（可记录 usage）
                Some("turn.completed") => {
                    if let Some(usage) = json.get("usage") {
                        tracing::debug!(usage = %usage, "Codex turn 完成");
                    }
                }

                _ => {
                    if line_count <= 10 {
                        tracing::debug!(type = ?type_str, "codex 事件（跳过）");
                    }
                }
            }
        }

        tracing::info!(
            total_lines = line_count,
            event_summary = ?event_counts,
            found_assistant_message = found_assistant,
            "codex stdout 读取完成",
        );

        // 确保子进程退出
        let exit_status = child.wait().await;
        tracing::debug!(exit_status = ?exit_status, "codex 子进程退出");
    });

    // 等待 session_id（来自 thread.started）
    let sid = tokio::time::timeout(std::time::Duration::from_secs(30), sid_rx)
        .await
        .map_err(|_| "等待 codex thread_id 超时 (30s)".to_string())?
        .map_err(|_| "无法从 codex 输出中提取 thread_id".to_string())?;

    tracing::debug!(session_id = %sid, "process_stream: codex thread_id 已提取");

    let stream: TextStream = Box::pin(ReceiverStream::new(text_rx));
    Ok((stream, sid))
}

/// 从 codex 的 assistant_message item.completed 事件中提取文本
///
/// 支持多种格式：
///
/// 新版（字段在 item 内）：
/// ```json
/// {"type":"item.completed","item":{"item_type":"assistant_message","content":[{"type":"text","text":"Hello!"}]}}
/// ```
///
/// 旧版（字段在顶层）：
/// ```json
/// {"type":"item.completed","item_type":"assistant_message","content":[{"type":"text","text":"Hello!"}]}
/// ```
///
/// 简化格式（text 直接是字符串）：
/// ```json
/// {"type":"item.completed","item":{"item_type":"assistant_message","text":"Hello!"}}
/// ```
fn extract_assistant_message_text(json: &serde_json::Value) -> Option<String> {
    // 新版：字段嵌套在 item 对象内；旧版：字段在顶层
    let target = json.get("item").unwrap_or(json);

    // 尝试内容数组格式：content 是对象数组
    if let Some(content) = target.get("content").and_then(|v| v.as_array()) {
        let mut text_parts = Vec::new();
        for block in content {
            if block.get("type").and_then(|v| v.as_str()) == Some("text") {
                if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                    if !text.is_empty() {
                        text_parts.push(text.to_string());
                    }
                }
            }
        }
        if !text_parts.is_empty() {
            return Some(text_parts.concat());
        }
    }

    // 尝试字符串格式：content 直接是字符串
    if let Some(text) = target.get("content").and_then(|v| v.as_str()) {
        if !text.is_empty() {
            return Some(text.to_string());
        }
    }

    // 尝试 text 字段
    if let Some(text) = target.get("text").and_then(|v| v.as_str()) {
        if !text.is_empty() {
            return Some(text.to_string());
        }
    }

    None
}

/// 检查 codex CLI 是否可用
async fn check_codex_available() -> bool {
    Command::from(haimen_core::process::build_command(
        "codex",
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

    #[test]
    fn test_extract_assistant_message_from_array() {
        let json = serde_json::json!({
            "type": "item.completed",
            "item_type": "assistant_message",
            "content": [
                {"type": "text", "text": "Hello, "},
                {"type": "text", "text": "world!"}
            ]
        });
        assert_eq!(
            extract_assistant_message_text(&json).as_deref(),
            Some("Hello, world!")
        );
    }

    #[test]
    fn test_extract_assistant_message_from_text_field() {
        let json = serde_json::json!({
            "type": "item.completed",
            "item_type": "assistant_message",
            "text": "Simple text response"
        });
        assert_eq!(
            extract_assistant_message_text(&json).as_deref(),
            Some("Simple text response")
        );
    }

    #[test]
    fn test_extract_assistant_message_from_string_content() {
        let json = serde_json::json!({
            "type": "item.completed",
            "item_type": "assistant_message",
            "content": "Direct string content"
        });
        assert_eq!(
            extract_assistant_message_text(&json).as_deref(),
            Some("Direct string content")
        );
    }

    #[test]
    fn test_extract_assistant_message_empty() {
        let json = serde_json::json!({
            "type": "item.completed",
            "item_type": "assistant_message",
            "content": []
        });
        assert!(extract_assistant_message_text(&json).is_none());
    }

    #[test]
    fn test_extract_assistant_message_no_text_blocks() {
        let json = serde_json::json!({
            "type": "item.completed",
            "item_type": "assistant_message",
            "content": [
                {"type": "tool_use", "name": "search"}
            ]
        });
        assert!(extract_assistant_message_text(&json).is_none());
    }

    #[test]
    fn test_extract_assistant_message_reasoning_ignored() {
        let json = serde_json::json!({
            "type": "item.completed",
            "item_type": "reasoning",
            "content": [{"type": "text", "text": "thinking..."}]
        });
        // 这个函数不会检查 item_type，但目前只有 assistant_message 才会被调用
        assert_eq!(
            extract_assistant_message_text(&json).as_deref(),
            Some("thinking...")
        );
    }

    #[test]
    fn test_codex_agent_name() {
        let agent = CodexAgent;
        assert_eq!(agent.name(), "codex");
    }

    #[test]
    fn test_extract_assistant_message_new_format_with_item_wrapper() {
        // 新版格式：字段在 item 对象内
        let json = serde_json::json!({
            "type": "item.completed",
            "item": {
                "id": "item_1",
                "item_type": "assistant_message",
                "content": [{"type": "text", "text": "New format response!"}]
            }
        });
        assert_eq!(
            extract_assistant_message_text(&json).as_deref(),
            Some("New format response!")
        );
    }

    #[test]
    fn test_extract_assistant_message_new_format_text_field() {
        // 新版格式：text 在 item 对象内
        let json = serde_json::json!({
            "type": "item.completed",
            "item": {
                "id": "item_1",
                "item_type": "assistant_message",
                "text": "New format text response"
            }
        });
        assert_eq!(
            extract_assistant_message_text(&json).as_deref(),
            Some("New format text response")
        );
    }

    #[test]
    fn test_extract_assistant_message_new_format_empty() {
        // 新版格式：空
        let json = serde_json::json!({
            "type": "item.completed",
            "item": {
                "id": "item_1",
                "item_type": "assistant_message",
                "content": []
            }
        });
        assert!(extract_assistant_message_text(&json).is_none());
    }

    #[test]
    fn test_extract_actual_codex_agent_message() {
        // Codex 实际输出的 agent_message 格式
        let json = serde_json::json!({
            "type": "item.completed",
            "item": {
                "id": "item_1",
                "type": "agent_message",
                "text": "你好！我是 haimen 的 AI 助手。"
            }
        });
        assert_eq!(
            extract_assistant_message_text(&json).as_deref(),
            Some("你好！我是 haimen 的 AI 助手。")
        );
    }

    #[test]
    fn test_extract_actual_codex_reasoning() {
        // Codex 实际输出的 reasoning 格式
        let json = serde_json::json!({
            "type": "item.completed",
            "item": {
                "id": "item_0",
                "type": "reasoning",
                "text": "The user wants me to introduce myself."
            }
        });
        assert_eq!(
            extract_assistant_message_text(&json).as_deref(),
            Some("The user wants me to introduce myself.")
        );
    }
}
