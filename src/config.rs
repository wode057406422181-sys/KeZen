use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// API endpoint URL
    pub api_url: Option<String>,

    /// API key (can also be set via INFINI_API_KEY env var)
    pub api_key: Option<String>,

    /// Model to use
    pub model: Option<String>,

    /// Maximum tokens for responses
    pub max_tokens: Option<u32>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            api_url: None,
            api_key: None,
            model: None,
            max_tokens: Some(8192),
        }
    }
}

impl AppConfig {
    /// Load configuration from the default config file
    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;
        if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            let config: AppConfig = toml::from_str(&content)?;
            Ok(config)
        } else {
            Ok(Self::default())
        }
    }

    /// Save configuration to the default config file
    pub fn save(&self) -> Result<()> {
        let path = Self::config_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        std::fs::write(&path, content)?;
        Ok(())
    }

    /// Get the default configuration file path
    fn config_path() -> Result<PathBuf> {
        let config_dir =
            dirs::config_dir().ok_or_else(|| anyhow::anyhow!("Cannot determine config directory"))?;
        Ok(config_dir.join("infini").join("config.toml"))
    }
}
