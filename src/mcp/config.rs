use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::fs;

/// Config mapped to ~/.kezen/mcp.json
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpConfig {
    #[serde(default, rename = "mcpServers")]
    pub servers: HashMap<String, McpServerConfig>,
    #[serde(default, rename = "allowedServers")]
    pub allowed_servers: Vec<String>,
    #[serde(default, rename = "deniedServers")]
    pub denied_servers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

impl McpConfig {
    /// Loads the configuration from ~/.kezen/mcp.json
    pub async fn load() -> Result<Option<Self>> {
        let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Home directory not found"))?;
        let config_path = home.join(".kezen").join("mcp.json");

        if !config_path.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(config_path).await?;
        let config = serde_json::from_str(&content)?;
        Ok(Some(config))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_deserialize_mcp_config() {
        let json_data = json!({
            "mcpServers": {
                "filesystem": {
                    "command": "node",
                    "args": ["-e", "println('hi')"]
                }
            },
            "allowedServers": ["filesystem"],
            "deniedServers": ["git"]
        });

        let config: McpConfig = serde_json::from_value(json_data).unwrap();
        assert_eq!(config.servers.len(), 1);
        assert!(config.servers.contains_key("filesystem"));
        assert_eq!(config.allowed_servers, vec!["filesystem".to_string()]);
        assert_eq!(config.denied_servers, vec!["git".to_string()]);
    }

    #[test]
    fn test_deserialize_mcp_config_defaults() {
        let json_data = json!({
            "mcpServers": {}
        });

        let config: McpConfig = serde_json::from_value(json_data).unwrap();
        assert!(config.allowed_servers.is_empty());
        assert!(config.denied_servers.is_empty());
    }
}

