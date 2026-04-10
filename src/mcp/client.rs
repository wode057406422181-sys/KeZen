use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

use crate::config::mcp::{McpConfig, McpServerConfig};
use super::tool::McpTool;
use super::transport::StdioTransport;
use crate::constants::api::MCP_PROTOCOL_VERSION;
use crate::tools::Tool;

#[derive(Debug, Clone)]
pub struct McpToolInfo {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub read_only_hint: bool,
}

pub struct McpClient {
    pub(crate) transport: StdioTransport,
    pub tools: Vec<McpToolInfo>,
}

impl McpClient {
    /// Connects to a server, performs handshake, and lists tools.
    pub async fn connect(name: &str, cfg: &McpServerConfig) -> Result<Self> {
        let transport = StdioTransport::spawn(cfg)
            .await
            .with_context(|| format!("Failed to spawn MCP server '{}'", name))?;

        // 1. initialize
        let init_req = json!({
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": {
                "roots": { "listChanged": true },
                "sampling": {}
            },
            "clientInfo": {
                "name": "kezen",
                "version": "0.1.0"
            }
        });

        // Use a timeout for initialize, as some servers might hang
        let _init_resp = tokio::time::timeout(
            Duration::from_secs(10),
            transport.request("initialize", init_req),
        )
        .await
        .context("Initialize request timed out")??;

        // 2. notifications/initialized
        transport
            .notify("notifications/initialized", json!({}))
            .await?;

        // 3. tools/list
        let tools_resp = transport.request("tools/list", json!({})).await?;

        let mut tools = Vec::new();
        if let Some(tools_arr) = tools_resp.get("tools").and_then(|t| t.as_array()) {
            for t in tools_arr {
                if let (Some(t_name), Some(t_desc), Some(t_schema)) = (
                    t.get("name").and_then(|n| n.as_str()),
                    t.get("description").and_then(|d| d.as_str()),
                    t.get("inputSchema"),
                ) {
                    let read_only_hint = t
                        .get("annotations")
                        .and_then(|a| a.get("readOnlyHint"))
                        .and_then(|r| r.as_bool())
                        .unwrap_or(false);

                    tools.push(McpToolInfo {
                        name: t_name.to_string(),
                        description: t_desc.to_string(),
                        input_schema: t_schema.clone(),
                        read_only_hint,
                    });
                }
            }
        }

        tracing::debug!(server = name, tools = tools.len(), "MCP handshake complete");

        Ok(Self { transport, tools })
    }

    /// Calls a specific tool on the server.
    ///
    /// Takes `&self` — the transport's `request()` method is concurrency-safe
    /// thanks to `AtomicU64` ID generation and channel-based I/O.
    pub async fn call_tool(&self, tool_name: &str, args: Value) -> Result<String> {
        let req = json!({
            "name": tool_name,
            "arguments": args
        });

        let resp = self.transport.request("tools/call", req).await?;

        if let Some(err) = resp.get("isError").and_then(|e| e.as_bool()) {
            if err {
                return Err(anyhow::anyhow!("Tool execution failed on server"));
            }
        }

        if let Some(content) = resp.get("content").and_then(|c| c.as_array()) {
            let mut output = String::new();
            for item in content {
                if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                    output.push_str(text);
                    output.push('\n');
                }
            }
            Ok(output.trim().to_string())
        } else {
            Ok(String::new())
        }
    }
}

/// Result of connecting to all configured MCP servers.
pub struct McpConnectResult {
    /// Tool trait objects ready for registration.
    pub tools: Vec<Arc<dyn Tool>>,
    /// Diagnostic messages for the frontend (replaces previous `eprintln!` calls).
    pub warnings: Vec<String>,
}

/// Connects to all servers defined in the config.
///
/// Returns tools + warnings instead of printing to stderr, so the frontend
/// can display connection status through the proper event channel.
pub async fn connect_all_servers() -> Result<McpConnectResult> {
    tracing::info!("Starting MCP server connections");
    let mut mcp_tools: Vec<Arc<dyn Tool>> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    // Fix #6: Match on all arms instead of silently swallowing Err
    match McpConfig::load().await {
        Ok(Some(config)) => {
            for (server_name, server_cfg) in config.servers {
                // Check deny list first
                if config.denied_servers.contains(&server_name) {
                    warnings.push(format!(
                        "[MCP] Skipping '{}': server is in denied list",
                        server_name
                    ));
                    continue;
                }

                // Check allow list if it's not empty
                if !config.allowed_servers.is_empty()
                    && !config.allowed_servers.contains(&server_name)
                {
                    warnings.push(format!(
                        "[MCP] Skipping '{}': server is not in allowed list",
                        server_name
                    ));
                    continue;
                }

                match McpClient::connect(&server_name, &server_cfg).await {
                    Ok(client) => {
                        // Fix #1: Use Arc<McpClient> directly instead of
                        // Arc<Mutex<McpClient>>.  call_tool() only needs &self
                        // and the transport is already concurrency-safe internally.
                        let client_arc = Arc::new(client);
                        let tools_info = client_arc.tools.clone();

                        for info in tools_info {
                            let wrapper = McpTool::new(&server_name, info, client_arc.clone());
                            mcp_tools.push(Arc::new(wrapper));
                        }
                    }
                    Err(e) => {
                        tracing::warn!(server = %server_name, error = %e, "MCP server connection failed");
                        warnings.push(format!(
                            "Failed to connect to MCP server '{}': {}",
                            server_name, e
                        ));
                    }
                }
            }
        }
        Ok(None) => {
            // No mcp.toml config file found, nothing to do
        }
        Err(e) => {
            warnings.push(format!("[MCP] Failed to load config: {}", e));
        }
    }

    Ok(McpConnectResult {
        tools: mcp_tools,
        warnings,
    })
}
