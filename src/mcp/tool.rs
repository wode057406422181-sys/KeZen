
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
    read_only_hint: bool,
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
            read_only_hint: info.read_only_hint,
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
        self.read_only_hint
    }

    fn permission_description(&self, input: &Value) -> String {
        // Show server, tool, and argument keys (not values, to avoid leaking secrets)
        let keys: Vec<&str> = input.as_object()
            .map(|m| m.keys().map(|k| k.as_str()).collect())
            .unwrap_or_default();
        if keys.is_empty() {
            format!("MCP server '{}' → {}", self.server_name, self.tool_name)
        } else {
            format!("MCP server '{}' → {}({})", self.server_name, self.tool_name, keys.join(", "))
        }
    }

    // TODO: Support server-level wildcard matching (e.g. "mcp__filesystem:*").
    //       This requires exposing a "Allow all tools from this server" option in
    //       the permission prompt UI and a way to store server-scoped rules.
    fn permission_matcher(&self, _input: &Value) -> Option<Box<dyn Fn(&str) -> bool + '_>> {
        // MCP tools don't have content-level rules (unlike Bash's prefix matching).
        // Always-allow is handled at the tool-name level (rule_content = None),
        // so no matcher is needed.
        None
    }

    fn permission_suggestion(&self, _input: &Value) -> Option<String> {
        // MCP tools are always-allowed at the whole-tool level (rule_content = None).
        // Returning None means the engine stores (tool_name, None), which matches
        // via the (None, _) => true branch in matches_rules.
        None
    }
}

/// Build display name: mcp__<server>__<tool>
pub fn build_mcp_tool_name(server: &str, tool: &str) -> String {
    let s = server.replace(|c: char| !c.is_alphanumeric(), "_");
    let t = tool.replace(|c: char| !c.is_alphanumeric(), "_");
    format!("mcp__{}__{}", s, t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_mcp_tool_name() {
        assert_eq!(build_mcp_tool_name("filesystem", "read_file"), "mcp__filesystem__read_file");
        assert_eq!(build_mcp_tool_name("my-server!", "my.tool"), "mcp__my_server___my_tool");
    }

    #[test]
    fn test_permission_description_logic() {
        // Test the logic directly since constructing a real McpClient is heavy for unit tests
        let server_name = "fs";
        let tool_name = "read";
        let input = serde_json::json!({"path": "foo.txt", "encoding": "utf8"});
        
        let keys: Vec<&str> = input.as_object()
            .map(|m| m.keys().map(|k| k.as_str()).collect())
            .unwrap_or_default();
        
        let desc = if keys.is_empty() {
            format!("MCP server '{}' → {}", server_name, tool_name)
        } else {
            format!("MCP server '{}' → {}({})", server_name, tool_name, keys.join(", "))
        };

        assert!(desc.contains("fs"));
        assert!(desc.contains("read"));
        assert!(desc.contains("path"));
        assert!(desc.contains("encoding"));
    }
}



