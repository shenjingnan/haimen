use async_trait::async_trait;

use crate::gateway::provider::{AgentOutput, AgentProvider};

use super::mcp_client::McpClient;

/// MCP 协议 Agent
///
/// 通过 MCP 协议连接 claude mcp serve 或其他 MCP 服务端。
pub struct McpAgent {
    client: McpClient,
}

impl McpAgent {
    pub fn new(command: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            client: McpClient::new(command, args),
        }
    }
}

#[async_trait]
impl AgentProvider for McpAgent {
    fn name(&self) -> &str {
        "mcp"
    }

    async fn process(
        &self,
        message: &str,
        _session_id: Option<&str>,
        _work_dir: &str,
    ) -> Result<(AgentOutput, String), String> {
        let response = self
            .client
            .call_agent(message, "haimen gateway")
            .await
            .map_err(|e| format!("MCP Agent 调用失败: {}", e))?;
        // MCP 协议级别无 session 概念，返回空字符串；无内容轨迹
        Ok((
            AgentOutput {
                text: response,
                events: Vec::new(),
            },
            String::new(),
        ))
    }

    async fn check_available(&self) -> Result<(), String> {
        if self.client.is_connected() {
            Ok(())
        } else {
            Err("MCP 客户端未连接".to_string())
        }
    }
}
