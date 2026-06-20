use async_trait::async_trait;

/// AI Agent 抽象
///
/// 所有 Agent 实现（Claude CLI、MCP Client、OpenAI 等）实现此 trait。
/// - process() 处理消息，支持会话 resume
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

    /// 检查是否可用（如 CLI 是否已安装、API key 是否有效）
    /// 返回 Err 时，chat_loop 应终止并报错
    async fn check_available(&self) -> Result<(), String>;
}
