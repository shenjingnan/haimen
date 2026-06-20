use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, CallToolResult, ListToolsResult, PaginatedRequestParams};
use rmcp::service::RunningService;
use rmcp::transport::child_process::TokioChildProcess;
use tokio::process::Command;
use tracing;

use crate::agents::error::{AgentError, AgentResult};

/// MCP 客户端，用于连接 MCP 服务端（如 claude mcp serve）
pub struct McpClient {
    /// 已连接的 service
    service: Option<RunningService<rmcp::service::RoleClient, ()>>,
    /// 可执行文件路径
    command: String,
    /// 启动参数
    args: Vec<String>,
}

impl McpClient {
    /// 创建新的 MCP 客户端
    pub fn new(command: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            service: None,
            command: command.into(),
            args,
        }
    }

    /// 连接到 MCP 服务端
    pub async fn connect(&mut self) -> AgentResult<()> {
        tracing::info!(command = %self.command, args = ?self.args, "正在连接 MCP 服务端");

        let mut cmd = Command::new(&self.command);
        cmd.args(&self.args);
        let child = TokioChildProcess::new(cmd)
            .map_err(|e| AgentError::Connection(format!("启动子进程失败: {}", e)))?;

        let service = ()
            .serve(child)
            .await
            .map_err(|e| AgentError::Connection(format!("MCP 握手失败: {}", e)))?;

        self.service = Some(service);
        tracing::info!("MCP 服务端已连接");
        Ok(())
    }

    /// 获取 service 引用
    fn service_ref(&self) -> AgentResult<&RunningService<rmcp::service::RoleClient, ()>> {
        self.service
            .as_ref()
            .ok_or_else(|| AgentError::Connection("MCP 客户端未连接".to_string()))
    }

    /// 列出可用工具
    pub async fn list_tools(&self) -> AgentResult<Vec<McpToolInfo>> {
        let result: ListToolsResult = self
            .service_ref()?
            .list_tools(None::<PaginatedRequestParams>)
            .await
            .map_err(|e| AgentError::Mcp(format!("list_tools 失败: {}", e)))?;

        let tools: Vec<McpToolInfo> = result
            .tools
            .into_iter()
            .map(|t| McpToolInfo {
                name: t.name.to_string(),
                description: t.description.unwrap_or_default().to_string(),
                input_schema: serde_json::Value::Object(t.input_schema.as_ref().clone()),
            })
            .collect();

        Ok(tools)
    }

    /// 调用工具
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: serde_json::Map<String, serde_json::Value>,
    ) -> AgentResult<CallToolResult> {
        tracing::info!(tool = %name, args_count = arguments.len(), "调用 MCP 工具");

        let params = CallToolRequestParams::new(name.to_string()).with_arguments(arguments);

        let result = self
            .service_ref()?
            .call_tool(params)
            .await
            .map_err(|e| AgentError::ToolCall(format!("工具 '{}' 调用失败: {}", name, e)))?;

        tracing::info!(
            tool = %name,
            content_items = result.content.len(),
            is_error = ?result.is_error,
            "MCP 工具调用完成"
        );

        Ok(result)
    }

    /// 调用 Agent 工具（claude mcp serve 的核心工具）
    ///
    /// 直接传用户消息，让 Claude Code 处理
    pub async fn call_agent(&self, prompt: &str, description: &str) -> AgentResult<String> {
        tracing::info!(
            prompt_len = prompt.len(),
            description = %description,
            "调用 MCP Agent 工具"
        );

        let mut args = serde_json::Map::new();
        args.insert(
            "prompt".to_string(),
            serde_json::Value::String(prompt.to_string()),
        );
        args.insert(
            "description".to_string(),
            serde_json::Value::String(description.to_string()),
        );

        let result = self.call_tool("Agent", args).await?;

        // 从结果中提取文本内容
        let text = extract_text_from_result(&result);

        tracing::info!(response_len = text.len(), "Agent 工具调用完成");
        Ok(text)
    }

    /// 断开连接
    pub async fn shutdown(&mut self) -> AgentResult<()> {
        if let Some(ref mut service) = self.service {
            service
                .close()
                .await
                .map_err(|e| AgentError::Other(format!("断开连接失败: {}", e)))?;
        }
        Ok(())
    }

    /// 检查是否已连接
    pub fn is_connected(&self) -> bool {
        self.service.is_some()
    }
}

/// 工具信息
#[derive(Debug, Clone)]
pub struct McpToolInfo {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// 从 CallToolResult 中提取文本内容
pub fn extract_text_from_result(result: &CallToolResult) -> String {
    let mut texts = Vec::new();

    for content in &result.content {
        // Content = Annotated<RawContent>
        let raw_value = serde_json::to_value(&content.raw).unwrap_or_default();
        if let Some(text) = raw_value.get("text").and_then(|v| v.as_str()) {
            texts.push(text.to_string());
        }
    }

    texts.join("\n")
}
