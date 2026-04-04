use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::tools::{Tool, ToolResult};
use super::client::{McpClient, McpToolInfo};

pub struct McpTool {
    display_name: String,     // "mcp__filesystem__read_file"
    server_name: String,
    tool_name: String,
    description: String,
    schema: Value,
    client: Arc<Mutex<McpClient>>,
}

impl McpTool {
    pub fn new(server_name: &str, info: McpToolInfo, client: Arc<Mutex<McpClient>>) -> Self {
        let display_name = build_mcp_tool_name(server_name, &info.name);
        Self {
            display_name,
            server_name: server_name.to_string(),
            tool_name: info.name,
            description: info.description,
            schema: info.input_schema,
            client,
        }
    }
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.display_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> Value {
        self.schema.clone()
    }

    async fn call(&self, input: Value) -> ToolResult {
        let mut client = self.client.lock().await;
        match client.call_tool(&self.tool_name, input).await {
            Ok(output) => ToolResult { content: output, is_error: false },
            Err(e) => ToolResult { content: e.to_string(), is_error: true },
        }
    }

    fn is_read_only(&self, _: &Value) -> bool {
        // Without deeper knowledge of the remote tool, we assume it's NOT read-only
        // (This triggers permission gating safely)
        false
    }
}

/// Build display name: mcp__<server>__<tool>
pub fn build_mcp_tool_name(server: &str, tool: &str) -> String {
    let s = server.replace(|c: char| !c.is_alphanumeric(), "_");
    let t = tool.replace(|c: char| !c.is_alphanumeric(), "_");
    format!("mcp__{}__{}", s, t)
}
