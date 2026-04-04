use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

use super::config::{McpConfig, McpServerConfig};
use super::transport::StdioTransport;
use super::tool::McpTool;
use crate::tools::Tool;

#[derive(Debug, Clone)]
pub struct McpToolInfo {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

pub struct McpClient {
    pub name: String,
    transport: StdioTransport,
    pub tools: Vec<McpToolInfo>,
}

impl McpClient {
    /// Connects to a server, performs handshake, and lists tools.
    pub async fn connect(name: &str, cfg: &McpServerConfig) -> Result<Self> {
        let mut transport = StdioTransport::spawn(cfg).await
            .with_context(|| format!("Failed to spawn MCP server '{}'", name))?;

        // 1. initialize
        let init_req = json!({
            "protocolVersion": "2024-11-05",
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
            transport.request("initialize", init_req)
        ).await.context("Initialize request timed out")??;

        // 2. notifications/initialized
        transport.notify("notifications/initialized", json!({})).await?;

        // 3. tools/list
        let tools_resp = transport.request("tools/list", json!({})).await?;
        
        let mut tools = Vec::new();
        if let Some(tools_arr) = tools_resp.get("tools").and_then(|t| t.as_array()) {
            for t in tools_arr {
                if let (Some(t_name), Some(t_desc), Some(t_schema)) = (
                    t.get("name").and_then(|n| n.as_str()),
                    t.get("description").and_then(|d| d.as_str()),
                    t.get("inputSchema")
                ) {
                    tools.push(McpToolInfo {
                        name: t_name.to_string(),
                        description: t_desc.to_string(),
                        input_schema: t_schema.clone(),
                    });
                }
            }
        }

        Ok(Self {
            name: name.to_string(),
            transport,
            tools,
        })
    }

    /// Calls a specific tool on the server.
    pub async fn call_tool(&mut self, tool_name: &str, args: Value) -> Result<String> {
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

    pub async fn shutdown(&mut self) {
        self.transport.shutdown().await;
    }
}

/// Helper function to connect to all servers defined in the config
pub async fn connect_all_servers() -> Result<Vec<Arc<dyn Tool>>> {
    let mut mcp_tools: Vec<Arc<dyn Tool>> = Vec::new();
    
    if let Ok(Some(config)) = McpConfig::load().await {
        for (server_name, server_cfg) in config.servers {
            match McpClient::connect(&server_name, &server_cfg).await {
                Ok(client) => {
                    let client_arc = Arc::new(tokio::sync::Mutex::new(client));
                    // Register each tool info into an McpTool wrapper
                    let tools_info = {
                        let c = client_arc.lock().await;
                        c.tools.clone()
                    };
                    
                    for info in tools_info {
                        let wrapper = McpTool::new(&server_name, info, client_arc.clone());
                        mcp_tools.push(Arc::new(wrapper));
                    }
                }
                Err(e) => {
                    eprintln!("Failed to connect to MCP server {}: {}", server_name, e);
                }
            }
        }
    }

    Ok(mcp_tools)
}
