use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

use super::client::{McpClient, McpToolInfo};
use crate::tools::{Tool, ToolResult};

pub struct McpTool {
    display_name: String, // "mcp__filesystem__read_file"
    server_name: String,
    tool_name: String,
    description: String,
    schema: Value,
    read_only_hint: bool,
    /// Fix #1: Use `Arc<McpClient>` instead of `Arc<Mutex<McpClient>>`.
    /// `call_tool()` only needs `&self` and the transport is internally
    /// concurrency-safe (AtomicU64 + channels), so no external lock is needed.
    client: Arc<McpClient>,
}

impl McpTool {
    pub fn new(server_name: &str, info: McpToolInfo, client: Arc<McpClient>) -> Self {
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
        // Fix #1: No Mutex lock needed — call_tool takes &self and the
        // transport handles concurrency internally via channels.
        match self.client.call_tool(&self.tool_name, input).await {
            Ok(output) => ToolResult::ok(output),
            Err(e) => {
                tracing::warn!(tool = %self.tool_name, server = %self.server_name, error = %e, "MCP tool call failed");
                ToolResult::err(e.to_string())
            }
        }
    }

    fn is_read_only(&self, _: &Value) -> bool {
        self.read_only_hint
    }

    fn permission_description(&self, input: &Value) -> String {
        // Show server, tool, and argument keys (not values, to avoid leaking secrets)
        let keys: Vec<&str> = input
            .as_object()
            .map(|m| m.keys().map(|k| k.as_str()).collect())
            .unwrap_or_default();
        if keys.is_empty() {
            format!("MCP server '{}' → {}", self.server_name, self.tool_name)
        } else {
            format!(
                "MCP server '{}' → {}({})",
                self.server_name,
                self.tool_name,
                keys.join(", ")
            )
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
///
/// Replaces non-alphanumeric characters with `_`, collapses consecutive
/// underscores, and strips leading/trailing underscores to avoid delimiter
/// confusion.
fn normalize_name(name: &str) -> String {
    let replaced: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    // Collapse consecutive underscores
    let mut result = String::with_capacity(replaced.len());
    let mut prev_underscore = false;
    for c in replaced.chars() {
        if c == '_' {
            if !prev_underscore {
                result.push(c);
            }
            prev_underscore = true;
        } else {
            result.push(c);
            prev_underscore = false;
        }
    }
    // Strip leading/trailing underscores
    result.trim_matches('_').to_string()
}

pub fn build_mcp_tool_name(server: &str, tool: &str) -> String {
    format!("mcp__{}__{}", normalize_name(server), normalize_name(tool))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_mcp_tool_name() {
        assert_eq!(
            build_mcp_tool_name("filesystem", "read_file"),
            "mcp__filesystem__read_file"
        );
        // Consecutive underscores collapsed, leading/trailing stripped
        assert_eq!(
            build_mcp_tool_name("my-server!", "my.tool"),
            "mcp__my-server__my_tool"
        );
    }

    #[test]
    fn test_normalize_name_collapses_underscores() {
        assert_eq!(normalize_name("a__b___c"), "a_b_c");
        assert_eq!(normalize_name("_leading_"), "leading");
        assert_eq!(normalize_name("normal-name"), "normal-name");
    }

    #[test]
    fn test_no_name_collision() {
        // "my-server" and "my_server" must NOT produce the same name
        // because hyphens are preserved
        let a = build_mcp_tool_name("my-server", "tool");
        let b = build_mcp_tool_name("my_server", "tool");
        assert_ne!(a, b);
    }

    #[test]
    fn test_permission_description_logic() {
        // Test the logic directly since constructing a real McpClient is heavy for unit tests
        let server_name = "fs";
        let tool_name = "read";
        let input = serde_json::json!({"path": "foo.txt", "encoding": "utf8"});

        let keys: Vec<&str> = input
            .as_object()
            .map(|m| m.keys().map(|k| k.as_str()).collect())
            .unwrap_or_default();

        let desc = if keys.is_empty() {
            format!("MCP server '{}' → {}", server_name, tool_name)
        } else {
            format!(
                "MCP server '{}' → {}({})",
                server_name,
                tool_name,
                keys.join(", ")
            )
        };

        assert!(desc.contains("fs"));
        assert!(desc.contains("read"));
        assert!(desc.contains("path"));
        assert!(desc.contains("encoding"));
    }
}
