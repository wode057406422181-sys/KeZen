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
