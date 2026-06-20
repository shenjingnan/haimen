#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("MCP 客户端错误: {0}")]
    Mcp(String),

    #[error("工具调用失败: {0}")]
    ToolCall(String),

    #[error("连接失败: {0}")]
    Connection(String),

    #[error("配置错误: {0}")]
    Config(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error("{0}")]
    Other(String),
}

pub type AgentResult<T> = Result<T, AgentError>;
