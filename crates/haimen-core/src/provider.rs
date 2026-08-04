use async_trait::async_trait;
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

/// 流式文本：Agent 逐块输出的文本流
pub type TextStream = Pin<Box<dyn Stream<Item = String> + Send>>;

/// 内容轨迹事件（按出现顺序记录，仅含 thinking / tool_use / tool_result）
///
/// text 事件不单独记录——文本增量仍经文本流聚合为 [`AgentOutput::text`]，
/// 避免与最终回复重复落盘。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentLogEvent {
    /// 思考块（模型推理过程）
    Thinking { thinking: String },
    /// 工具调用（含最终拼接好的入参 JSON）
    ToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: serde_json::Value,
    },
    /// 工具执行结果
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default)]
        is_error: bool,
    },
}

/// Agent 单次调用的完整输出
#[derive(Debug, Clone, Default)]
pub struct AgentOutput {
    /// 最终文本回复（text_delta 拼接，与旧 process() 返回一致）
    pub text: String,
    /// 完整内容轨迹（thinking / tool_use / tool_result），按出现顺序
    pub events: Vec<AgentLogEvent>,
}

/// 流结束后投递完整事件轨迹的接收端
///
/// 实现 `process_stream` 时，后台读流任务在读到 EOF 后经 oneshot 发送
/// 完整 `events`；调用方可在排空文本流后 `.await` 取回。
pub type AgentEventReceiver = tokio::sync::oneshot::Receiver<Vec<AgentLogEvent>>;

/// AI Agent 抽象
///
/// 所有 Agent 实现（Claude CLI、MCP Client、OpenAI 等）实现此 trait。
/// - process() 处理消息，支持会话 resume，返回完整输出（文本 + 内容轨迹）
/// - process_stream() 流式处理消息，逐块返回文本（默认实现回退到 process()）
#[async_trait]
pub trait AgentProvider: Send + Sync {
    /// Agent 名称
    fn name(&self) -> &str;

    /// 处理消息
    ///
    /// - message: 用户消息文本
    /// - session_id: Some(id) 表示继续已有会话，None 表示新会话
    /// - work_dir: Agent 子进程的工作目录
    /// 返回: (完整输出, 新的 session_id)
    async fn process(
        &self,
        message: &str,
        session_id: Option<&str>,
        work_dir: &str,
    ) -> Result<(AgentOutput, String), String>;

    /// 流式处理消息
    ///
    /// 逐块返回 Agent 输出文本，适用于驱动流式 TTS。
    /// 默认实现将 process() 的完整结果作为一个块返回，事件经 oneshot 立即投递。
    ///
    /// 返回: (文本流, 新的 session_id, 事件轨迹接收端)
    async fn process_stream(
        &self,
        message: &str,
        session_id: Option<&str>,
        work_dir: &str,
    ) -> Result<(TextStream, String, AgentEventReceiver), String> {
        let (output, sid) = self.process(message, session_id, work_dir).await?;
        let stream: TextStream = Box::pin(futures_util::stream::once(async move { output.text }));
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = tx.send(output.events);
        Ok((stream, sid, rx))
    }

    /// 检查是否可用（如 CLI 是否已安装、API key 是否有效）
    /// 返回 Err 时，chat_loop 应终止并报错
    async fn check_available(&self) -> Result<(), String>;
}
