use std::fmt;
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    #[default]
    Anthropic,
    OpenAi,
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Provider::Anthropic => write!(f, "anthropic"),
            Provider::OpenAi => write!(f, "openai"),
        }
    }
}

/// Application configuration
///
/// Loading priority (high → low):
/// 1. CLI arguments (--model, --api-key, etc.)
/// 2. INFINI_* environment variables
/// 3. ANTHROPIC_API_KEY / OPENAI_API_KEY (auto-detect provider)
/// 4. Config file (~/.config/infini/config.toml)
/// 5. Defaults (only max_tokens = 8192; model has no default)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub provider: Provider,

    /// Custom API endpoint URL
    pub api_url: Option<String>,

    /// API key
    pub api_key: Option<String>,

    /// Model to use (no default; user must specify)
    pub model: Option<String>,

    /// Maximum tokens for responses
    pub max_tokens: Option<u32>,

    /// Enable extended thinking (Anthropic only)
    #[serde(default)]
    pub thinking: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            provider: Provider::Anthropic,
            api_url: None,
            api_key: None,
            model: None,
            max_tokens: Some(8192),
            thinking: false,
        }
    }
}

impl AppConfig {
    /// Load configuration from the default config file, applying ENV overrides.
    ///
    /// Priority: config file < ANTHROPIC/OPENAI env < INFINI_* env
    /// (CLI overrides are applied in main.rs after this call)
    pub fn load() -> Result<Self> {
        let mut config = Self::default();

        // Layer 4: config file
        let path = Self::config_path()?;
        if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            if let Ok(file_config) = toml::from_str::<AppConfig>(&content) {
                config = file_config;
                // Ensure defaults for missing optional fields
                if config.max_tokens.is_none() {
                    config.max_tokens = Some(8192);
                }
            }
        }

        // Layer 3: fallback provider-specific env vars (lower priority)
        if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
            if config.api_key.is_none() {
                config.api_key = Some(key);
                config.provider = Provider::Anthropic;
            }
        } else if let Ok(key) = std::env::var("OPENAI_API_KEY")
            && config.api_key.is_none()
        {
            config.api_key = Some(key);
            config.provider = Provider::OpenAi;
        }

        // Layer 2: INFINI_* env vars (higher priority, override everything above)
        if let Ok(val) = std::env::var("INFINI_PROVIDER") {
            config.provider = match val.to_lowercase().as_str() {
                "openai" => Provider::OpenAi,
                _ => Provider::Anthropic,
            };
        }
        if let Ok(val) = std::env::var("INFINI_API_KEY") {
            config.api_key = Some(val);
        }
        if let Ok(val) = std::env::var("INFINI_BASE_URL") {
            config.api_url = Some(val);
        }
        if let Ok(val) = std::env::var("INFINI_MODEL") {
            config.model = Some(val);
        }

        Ok(config)
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
        let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
        Ok(home.join(".config").join("infini").join("config.toml"))
    }

    /// Get the base URL for the configured provider
    pub fn base_url(&self) -> &str {
        self.api_url.as_deref().unwrap_or(match self.provider {
            Provider::Anthropic => "https://api.anthropic.com",
            Provider::OpenAi => "https://api.openai.com",
        })
    }
}
