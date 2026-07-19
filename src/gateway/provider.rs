use async_trait::async_trait;
use futures_util::Stream;
use std::pin::Pin;

/// 流式文本：Agent 逐块输出的文本流
pub type TextStream = Pin<Box<dyn Stream<Item = String> + Send>>;

/// AI Agent 抽象
///
/// 所有 Agent 实现（Claude CLI、MCP Client、OpenAI 等）实现此 trait。
/// - process() 处理消息，支持会话 resume，返回完整文本
/// - process_stream() 流式处理消息，逐块返回文本（默认实现回退到 process()）
#[async_trait]
pub trait AgentProvider: Send + Sync {
    /// Agent 名称
    fn name(&self) -> &str;

    /// 处理消息
    ///
    /// - message: 用户消息文本
    /// - session_id: Some(id) 表示继续已有会话，None 表示新会话
    /// 返回: (完整回复文本, 新的 session_id)
    async fn process(
        &self,
        message: &str,
        session_id: Option<&str>,
    ) -> Result<(String, String), String>;

    /// 流式处理消息
    ///
    /// 逐块返回 Agent 输出文本，适用于驱动流式 TTS。
    /// 默认实现将 process() 的完整结果作为一个块返回。
    ///
    /// 返回: (文本流, 新的 session_id)
    async fn process_stream(
        &self,
        message: &str,
        session_id: Option<&str>,
    ) -> Result<(TextStream, String), String> {
        let (text, sid) = self.process(message, session_id).await?;
        let stream: TextStream = Box::pin(futures_util::stream::once(async move { text }));
        Ok((stream, sid))
    }

    /// 检查是否可用（如 CLI 是否已安装、API key 是否有效）
    /// 返回 Err 时，chat_loop 应终止并报错
    async fn check_available(&self) -> Result<(), String>;
}
