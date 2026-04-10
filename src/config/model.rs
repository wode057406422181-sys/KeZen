use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::{Provider, default_true};

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
}
