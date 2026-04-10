use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::{Provider, default_true};

use crate::constants::api::DEFAULT_MAX_TOKENS;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelProfile {
    #[serde(default)]
    pub provider: Provider,
    pub model: String,
    pub api_key: Option<String>,
    pub api_url: Option<String>,
    #[serde(default)]
    pub thinking: bool,
    #[serde(default = "default_true")]
    pub include_stream_usage: bool,
    #[serde(default = "default_true")]
    pub enable_cache: bool,
    /// Max output tokens for this model. Defaults to DEFAULT_MAX_TOKENS.
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    /// Context window size for this model (tokens).
    pub context_window: Option<u64>,
    /// Custom User-Agent string for this model's HTTP requests.
    pub user_agent: Option<String>,
}

fn default_max_tokens() -> u32 {
    DEFAULT_MAX_TOKENS
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelsConfig {
    #[serde(default)]
    pub models: HashMap<String, ModelProfile>,
}

impl ModelsConfig {
    /// Get the default path to model.toml
    pub fn config_path() -> Result<PathBuf> {
        let home =
            dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
        Ok(home.join(".kezen").join("config").join("model.toml"))
    }

    /// Load the models dictionary from model.toml
    /// Note: `kezen.toml` models are also supported but they are parsed
    /// directly by the `AppConfig` parser and merged.
    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;
        if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            let config: ModelsConfig = toml::from_str(&content).map_err(|e| {
                anyhow::anyhow!("Failed to parse model file at {}: {}", path.display(), e)
            })?;
            Ok(config)
        } else {
            Ok(Self::default())
        }
    }

    /// Save the models dictionary to model.toml
    pub fn save(&self) -> Result<()> {
        let path = Self::config_path()?;
        
        let parent = path.parent().unwrap();
        if !parent.exists() {
            std::fs::create_dir_all(parent)?;
        }
        
        let content = toml::to_string_pretty(self)?;
        std::fs::write(&path, content)?;
        Ok(())
    }
}
