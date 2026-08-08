use async_trait::async_trait;
use futures_util::StreamExt;
use haimen_core::provider::{
    AgentEventStream, AgentLogEvent, AgentOutput, AgentProvider, TextStream,
};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::ReceiverStream;
use tracing;

/// Claude Code Agent
///
/// 通过 `claude --print` 子进程调用 Claude Code 处理消息。
/// 同时支持批处理（process）和流式处理（process_stream）。
pub struct ClaudeAgent {
    /// claude CLI 可执行文件路径（默认 "claude"，由 build_command 按 PATH 查找）
    cli_path: String,
}

impl ClaudeAgent {
    /// 使用指定 CLI 路径构造（空串/裸命令名时由 `build_command` 按 PATH 解析）
    pub fn new(cli_path: impl Into<String>) -> Self {
        Self {
            cli_path: cli_path.into(),
        }
    }

    /// 当前 CLI 路径（供测试断言使用）
    #[cfg(test)]
    pub(crate) fn cli_path(&self) -> &str {
        &self.cli_path
    }
}

#[async_trait]
impl AgentProvider for ClaudeAgent {
    fn name(&self) -> &str {
        "claude-code"
    }

    /// 批处理：等待 claude 全部输出完成后返回完整文本与内容轨迹
    async fn process(
        &self,
        message: &str,
        session_id: Option<&str>,
        work_dir: &str,
    ) -> Result<(AgentOutput, String), String> {
        let (mut stream, sid, events_rx) =
            self.process_stream(message, session_id, work_dir).await?;
        let mut full_text = String::new();
        while let Some(chunk) = stream.next().await {
            full_text.push_str(&chunk);
        }
        let final_text = full_text.trim().to_string();
        if final_text.is_empty() {
            return Err("Claude 返回为空".to_string());
        }
        // 排空实时事件流：后台任务读完所有输出后 drop sender，recv 返回 None。
        // 加超时防御边缘场景（如消费者提前停止导致读流任务阻塞在发送上）。
        let mut events_rx = events_rx;
        let mut events = Vec::new();
        while let Ok(Some(event)) =
            tokio::time::timeout(std::time::Duration::from_secs(5), events_rx.recv()).await
        {
            events.push(event);
        }
        Ok((
            AgentOutput {
                text: final_text,
                events,
            },
            sid,
        ))
    }

    /// 流式处理：逐块返回 claude 的文本输出，实现 Agent 输出与 TTS 合成的并行
    async fn process_stream(
        &self,
        message: &str,
        session_id: Option<&str>,
        work_dir: &str,
    ) -> Result<(TextStream, String, AgentEventStream), String> {
        process_with_claude_stream(message, session_id, work_dir, &self.cli_path).await
    }

    async fn check_available(&self) -> Result<(), String> {
        if check_claude_available(&self.cli_path).await {
            Ok(())
        } else {
            Err(format!(
                "claude CLI 不可用（路径: {}）。请检查 cli_path 配置或执行: npm install -g @anthropic-ai/claude-code",
                self.cli_path
            ))
        }
    }
}

/// 单个内容块的累积状态（`content_block_start` 建立，`content_block_stop` 终结）
enum BlockAcc {
    /// 助手文本块（不产生事件，文本直接走 TTS 通道）
    Text,
    /// 思考块
    Thinking { thinking: String },
    /// 工具调用块（入参以 `input_json_delta` 增量到达）
    ToolUse {
        id: String,
        name: String,
        partial_json: String,
    },
    /// 工具结果块
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
}

/// 启动 claude --print 子进程，返回文本流、session_id 与实时事件流
async fn process_with_claude_stream(
    prompt: &str,
    resume_session_id: Option<&str>,
    work_dir: &str,
    cli_path: &str,
) -> Result<(TextStream, String, AgentEventStream), String> {
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

    // Windows 上 claude 是 npm 安装的 .cmd shim，需经 build_command 解析包装
    let mut child = Command::from(haimen_core::process::build_command(cli_path, &args))
        .current_dir(work_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("启动 claude 失败: {}", e))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "无法获取 claude stdout".to_string())?;

    // 三个通道：session_id（oneshot）+ 文本流（mpsc）+ 实时事件流（mpsc）
    let (sid_tx, sid_rx) = oneshot::channel::<String>();
    let (events_tx, events_rx) = mpsc::channel::<AgentLogEvent>(64);
    let (text_tx, text_rx) = mpsc::channel::<String>(64);

    // 后台读取任务：解析 JSON 行，提取文本块发送到文本通道，
    // 同时把 thinking / tool_use / tool_result 事件实时转发到事件流
    tokio::spawn(async move {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();
        let mut system_init_parsed = false;
        let mut sid_tx = Some(sid_tx);
        let mut events: Vec<AgentLogEvent> = Vec::new();
        let events_tx = Some(events_tx);
        let mut current: Option<BlockAcc> = None;

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
                    // 返回 false 表示文本通道已关闭（消费者停止）→ 退出读取循环
                    let keep_going =
                        handle_stream_event(&json, &text_tx, &mut current, &mut events, &events_tx)
                            .await;
                    if !keep_going {
                        break;
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

        // 事件流 sender 在此 drop → 消费者 recv() 返回 None（事件流结束）
        drop(events_tx);
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
    Ok((stream, sid, events_rx))
}

/// 记录一个 [`AgentLogEvent`]：累积到 `events` 备用，若 `events_tx` 为 Some 则实时转发
async fn record_event(
    events: &mut Vec<AgentLogEvent>,
    events_tx: &Option<mpsc::Sender<AgentLogEvent>>,
    event: AgentLogEvent,
) {
    if let Some(tx) = events_tx {
        let _ = tx.send(event.clone()).await;
    }
    events.push(event);
}

/// 处理一行 `stream_event`，返回 false 表示文本通道已关闭（调用方应退出读取循环）
///
/// 除 text_delta 外，`thinking_delta` / `input_json_delta` / `content_block_start|stop`
/// 均被累积为 [`AgentLogEvent`] 内容轨迹，并（若提供 `events_tx`）实时转发到事件流。
async fn handle_stream_event(
    json: &serde_json::Value,
    text_tx: &mpsc::Sender<String>,
    current: &mut Option<BlockAcc>,
    events: &mut Vec<AgentLogEvent>,
    events_tx: &Option<mpsc::Sender<AgentLogEvent>>,
) -> bool {
    // 嵌套格式: {"event": {"type": "content_block_delta", "delta": ...}}
    let Some(event) = json.get("event") else {
        // 扁平格式兼容：仅 text_delta
        // 形如 {"type": "stream_event", "event_type": "content_block_delta", "delta": ...}
        if let Some(text) = extract_text_from_flat(json) {
            return text_tx.send(text).await.is_ok();
        }
        return true;
    };

    let Some(etype) = event.get("type").and_then(|v| v.as_str()) else {
        return true;
    };

    match etype {
        "content_block_start" => {
            let Some(block) = event.get("content_block") else {
                return true;
            };
            let Some(bt) = block.get("type").and_then(|v| v.as_str()) else {
                return true;
            };
            *current = match bt {
                "text" => Some(BlockAcc::Text),
                "thinking" => Some(BlockAcc::Thinking {
                    thinking: str_field(block, "thinking"),
                }),
                "tool_use" => Some(BlockAcc::ToolUse {
                    id: str_field(block, "id"),
                    name: str_field(block, "name"),
                    partial_json: String::new(),
                }),
                "tool_result" => Some(BlockAcc::ToolResult {
                    tool_use_id: str_field(block, "tool_use_id"),
                    content: String::new(),
                    is_error: block
                        .get("is_error")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                }),
                _ => None,
            };
            true
        }

        "content_block_delta" => {
            let Some(delta) = event.get("delta") else {
                return true;
            };
            let Some(dtype) = delta.get("type").and_then(|v| v.as_str()) else {
                return true;
            };
            match dtype {
                "text_delta" => {
                    let text = delta
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default();
                    if text.is_empty() {
                        return true;
                    }
                    match current {
                        // 助手文本块（或未知状态）→ 走 TTS/输出通道（与旧行为一致）
                        Some(BlockAcc::Text) | None => {
                            return text_tx.send(text.to_string()).await.is_ok();
                        }
                        // 工具结果块 → 内容并入事件
                        Some(BlockAcc::ToolResult { content, .. }) => {
                            content.push_str(text);
                        }
                        _ => {}
                    }
                }
                "thinking_delta" => {
                    if let Some(BlockAcc::Thinking { thinking }) = current {
                        if let Some(t) = delta.get("thinking").and_then(|v| v.as_str()) {
                            thinking.push_str(t);
                        }
                    }
                }
                "input_json_delta" => {
                    if let Some(BlockAcc::ToolUse { partial_json, .. }) = current {
                        if let Some(pj) = delta.get("partial_json").and_then(|v| v.as_str()) {
                            partial_json.push_str(pj);
                        }
                    }
                }
                _ => {} // signature_delta 等忽略
            }
            true
        }

        "content_block_stop" => {
            if let Some(acc) = current.take() {
                match acc {
                    BlockAcc::Text => {}
                    BlockAcc::Thinking { thinking } => {
                        tracing::info!(
                            agent = "claude-code",
                            event = "thinking",
                            thinking = %thinking,
                            "Claude 思考",
                        );
                        record_event(events, events_tx, AgentLogEvent::Thinking { thinking }).await;
                    }
                    BlockAcc::ToolUse {
                        id,
                        name,
                        partial_json,
                    } => {
                        let input =
                            serde_json::from_str(&partial_json).unwrap_or(serde_json::Value::Null);
                        tracing::info!(
                            agent = "claude-code",
                            event = "tool_use",
                            id = %id,
                            name = %name,
                            input = %input,
                            "Claude 调用工具",
                        );
                        record_event(
                            events,
                            events_tx,
                            AgentLogEvent::ToolUse { id, name, input },
                        )
                        .await;
                    }
                    BlockAcc::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => {
                        tracing::info!(
                            agent = "claude-code",
                            event = "tool_result",
                            tool_use_id = %tool_use_id,
                            is_error,
                            content = %content,
                            "Claude 工具结果",
                        );
                        record_event(
                            events,
                            events_tx,
                            AgentLogEvent::ToolResult {
                                tool_use_id,
                                content,
                                is_error,
                            },
                        )
                        .await;
                    }
                }
            }
            true
        }

        _ => true,
    }
}

/// 从 JSON 对象取字符串字段，缺省为空串
fn str_field(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string()
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
async fn check_claude_available(cli_path: &str) -> bool {
    Command::from(haimen_core::process::build_command(
        cli_path,
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

    /// 构造一行嵌套格式的 stream_event JSON
    fn event_line(event: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "type": "stream_event", "event": event })
    }

    /// 驱动 handle_stream_event 处理若干行，返回收集到的文本块与事件
    async fn run_events(lines: &[serde_json::Value]) -> (Vec<String>, Vec<AgentLogEvent>) {
        let (text_tx, mut text_rx) = mpsc::channel::<String>(64);
        let mut current = None;
        let mut events = Vec::new();
        for line in lines {
            let ok = handle_stream_event(line, &text_tx, &mut current, &mut events, &None).await;
            assert!(ok, "文本通道不应提前关闭");
        }
        let mut texts = Vec::new();
        while let Ok(t) = text_rx.try_recv() {
            texts.push(t);
        }
        (texts, events)
    }

    #[tokio::test]
    async fn test_capture_thinking() {
        let lines = [
            event_line(serde_json::json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": { "type": "thinking", "thinking": "", "signature": "sig1" }
            })),
            event_line(serde_json::json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "thinking_delta", "thinking": "让我想想，" }
            })),
            event_line(serde_json::json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "signature_delta", "signature": "sig2" }
            })),
            event_line(serde_json::json!({ "type": "content_block_stop", "index": 0 })),
        ];
        let (texts, events) = run_events(&lines).await;
        assert!(texts.is_empty(), "思考块不应产生输出文本");
        assert_eq!(
            events,
            vec![AgentLogEvent::Thinking {
                thinking: "让我想想，".to_string()
            }]
        );
    }

    #[tokio::test]
    async fn test_capture_tool_use_and_result() {
        let lines = [
            event_line(serde_json::json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": { "type": "tool_use", "id": "toolu_1", "name": "Read", "input": {} }
            })),
            event_line(serde_json::json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "input_json_delta", "partial_json": "{\"file_path\":" }
            })),
            event_line(serde_json::json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "input_json_delta", "partial_json": "\"/tmp/a.txt\"}" }
            })),
            event_line(serde_json::json!({ "type": "content_block_stop", "index": 0 })),
            event_line(serde_json::json!({
                "type": "content_block_start",
                "index": 1,
                "content_block": { "type": "tool_result", "tool_use_id": "toolu_1", "content": [], "is_error": false }
            })),
            event_line(serde_json::json!({
                "type": "content_block_delta",
                "index": 1,
                "delta": { "type": "text_delta", "text": "file content" }
            })),
            event_line(serde_json::json!({ "type": "content_block_stop", "index": 1 })),
        ];
        let (texts, events) = run_events(&lines).await;
        // 工具结果文本不应进入输出流
        assert!(texts.is_empty(), "工具结果不应进入输出文本");
        assert_eq!(
            events,
            vec![
                AgentLogEvent::ToolUse {
                    id: "toolu_1".to_string(),
                    name: "Read".to_string(),
                    input: serde_json::json!({ "file_path": "/tmp/a.txt" }),
                },
                AgentLogEvent::ToolResult {
                    tool_use_id: "toolu_1".to_string(),
                    content: "file content".to_string(),
                    is_error: false,
                },
            ]
        );
    }

    #[tokio::test]
    async fn test_capture_text_goes_to_stream() {
        let lines = [
            event_line(serde_json::json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": { "type": "text", "text": "" }
            })),
            event_line(serde_json::json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "text_delta", "text": "你好，" }
            })),
            event_line(serde_json::json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "text_delta", "text": "世界！" }
            })),
            event_line(serde_json::json!({ "type": "content_block_stop", "index": 0 })),
        ];
        let (texts, events) = run_events(&lines).await;
        assert_eq!(texts, vec!["你好，".to_string(), "世界！".to_string()]);
        assert!(events.is_empty(), "文本块不应产生事件");
    }

    #[tokio::test]
    async fn test_mixed_ordering_preserved() {
        let lines = [
            event_line(serde_json::json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": { "type": "thinking", "thinking": "", "signature": "s" }
            })),
            event_line(serde_json::json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "thinking_delta", "thinking": "先看代码" }
            })),
            event_line(serde_json::json!({ "type": "content_block_stop", "index": 0 })),
            event_line(serde_json::json!({
                "type": "content_block_start",
                "index": 1,
                "content_block": { "type": "tool_use", "id": "toolu_2", "name": "Bash", "input": {} }
            })),
            event_line(serde_json::json!({
                "type": "content_block_delta",
                "index": 1,
                "delta": { "type": "input_json_delta", "partial_json": "{\"command\":\"ls\"}" }
            })),
            event_line(serde_json::json!({ "type": "content_block_stop", "index": 1 })),
            event_line(serde_json::json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": { "type": "text", "text": "" }
            })),
            event_line(serde_json::json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "text_delta", "text": "完成" }
            })),
            event_line(serde_json::json!({ "type": "content_block_stop", "index": 0 })),
        ];
        let (texts, events) = run_events(&lines).await;
        assert_eq!(texts, vec!["完成".to_string()]);
        assert_eq!(
            events,
            vec![
                AgentLogEvent::Thinking {
                    thinking: "先看代码".to_string()
                },
                AgentLogEvent::ToolUse {
                    id: "toolu_2".to_string(),
                    name: "Bash".to_string(),
                    input: serde_json::json!({ "command": "ls" }),
                },
            ]
        );
    }

    #[tokio::test]
    async fn test_flat_format_text() {
        let line = serde_json::json!({
            "type": "stream_event",
            "event_type": "content_block_delta",
            "delta": { "type": "text_delta", "text": "扁平格式" }
        });
        let (texts, events) = run_events(&[line]).await;
        assert_eq!(texts, vec!["扁平格式".to_string()]);
        assert!(events.is_empty());
    }
}
