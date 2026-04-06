use anyhow::Result;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::fs;

/// Config mapped to ~/.kezen/config/mcp.toml
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpConfig {
    /// `IndexMap` preserves insertion order from the TOML file.
    /// Servers appear in the tool schema in the order the user wrote them,
    /// which is both deterministic and intuitive.
    #[serde(default)]
    pub servers: IndexMap<String, McpServerConfig>,
    #[serde(default)]
    pub allowed_servers: Vec<String>,
    #[serde(default)]
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
    /// Loads the configuration from ~/.kezen/config/mcp.toml.
    ///
    /// Uses fully-async I/O (no blocking `path.exists()`) per project conventions.
    pub async fn load() -> Result<Option<Self>> {
        let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Home directory not found"))?;
        let config_path = home.join(".kezen").join("config").join("mcp.toml");

        match fs::read_to_string(&config_path).await {
            Ok(content) => {
                let config = toml::from_str(&content)?;
                Ok(Some(config))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_mcp_config() {
        let toml_str = r#"
allowed_servers = ["filesystem"]
denied_servers = ["git"]

[servers.filesystem]
command = "node"
args = ["-e", "println('hi')"]
"#;

        let config: McpConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.servers.len(), 1);
        assert!(config.servers.contains_key("filesystem"));
        assert_eq!(config.allowed_servers, vec!["filesystem".to_string()]);
        assert_eq!(config.denied_servers, vec!["git".to_string()]);
    }

    #[test]
    fn test_deserialize_mcp_config_defaults() {
        let toml_str = "";

        let config: McpConfig = toml::from_str(toml_str).unwrap();
        assert!(config.servers.is_empty());
        assert!(config.allowed_servers.is_empty());
        assert!(config.denied_servers.is_empty());
    }

    #[test]
    fn test_deserialize_mcp_config_with_env() {
        let toml_str = r#"
[servers.my-server]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem"]

[servers.my-server.env]
HOME = "/tmp"
NODE_ENV = "production"
"#;

        let config: McpConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.servers.len(), 1);
        let server = config.servers.get("my-server").unwrap();
        assert_eq!(server.command, "npx");
        assert_eq!(server.env.get("NODE_ENV").unwrap(), "production");
    }
}
