use std::fmt;
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::constants::defaults::{
    DEFAULT_ANTHROPIC_BASE_URL, DEFAULT_MAX_TOKENS, DEFAULT_OPENAI_BASE_URL, DEFAULT_USER_AGENT,
};

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
/// 2. KEZEN_* environment variables
/// 3. ANTHROPIC_API_KEY / OPENAI_API_KEY (auto-detect provider)
/// 4. Config file (~/.kezen/config/config.toml)
/// 5. Defaults (only max_tokens = 8192; model has no default)
#[derive(Clone, Serialize, Deserialize)]
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

    /// Custom User-Agent header (useful for Coding Plan endpoints)
    pub user_agent: Option<String>,

    /// Send stream_options.include_usage in OpenAI streaming requests.
    ///
    /// Set to `false` for endpoints that don't support this field (DashScope,
    /// Ollama, vLLM, etc.). Defaults to `true` for the official OpenAI API.
    #[serde(default = "default_true")]
    pub include_stream_usage: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            provider: Provider::Anthropic,
            api_url: None,
            api_key: None,
            model: None,
            max_tokens: Some(DEFAULT_MAX_TOKENS),
            thinking: false,
            user_agent: None,
            include_stream_usage: true,
        }
    }
}

fn default_true() -> bool {
    true
}

impl AppConfig {
    /// Load configuration from the default config file, applying ENV overrides.
    ///
    /// Priority: config file < ANTHROPIC/OPENAI env < KEZEN_* env
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

        // Layer 2: KEZEN_* env vars (higher priority, override everything above)
        if let Ok(val) = std::env::var("KEZEN_PROVIDER") {
            config.provider = match val.to_lowercase().as_str() {
                "openai" => Provider::OpenAi,
                _ => Provider::Anthropic,
            };
        }
        if let Ok(val) = std::env::var("KEZEN_API_KEY") {
            config.api_key = Some(val);
        }
        if let Ok(val) = std::env::var("KEZEN_BASE_URL") {
            config.api_url = Some(val);
        }
        if let Ok(val) = std::env::var("KEZEN_MODEL") {
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
    pub fn config_path() -> Result<PathBuf> {
        let home =
            dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
        Ok(home.join(".kezen").join("config").join("config.toml"))
    }

    /// Get the base URL for the configured provider
    pub fn base_url(&self) -> &str {
        self.api_url.as_deref().unwrap_or(match self.provider {
            Provider::Anthropic => DEFAULT_ANTHROPIC_BASE_URL,
            Provider::OpenAi => DEFAULT_OPENAI_BASE_URL,
        })
    }

    /// Get User-Agent string (configurable, defaults to kezen/<version>)
    pub fn user_agent(&self) -> &str {
        self.user_agent.as_deref().unwrap_or(DEFAULT_USER_AGENT)
    }
}

impl fmt::Debug for AppConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppConfig")
            .field("provider", &self.provider)
            .field("api_url", &self.api_url)
            // Redact the API key — never print credentials to the terminal.
            .field(
                "api_key",
                &self.api_key.as_deref().map(|_| "[REDACTED]"),
            )
            .field("model", &self.model)
            .field("max_tokens", &self.max_tokens)
            .field("thinking", &self.thinking)
            .field("user_agent", &self.user_agent)
            .field("include_stream_usage", &self.include_stream_usage)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> AppConfig {
        AppConfig::default()
    }

    // ── Defaults ─────────────────────────────────────────────────────────────

    #[test]
    fn default_provider_is_anthropic() {
        assert_eq!(default_config().provider, Provider::Anthropic);
    }

    #[test]
    fn default_max_tokens_is_8192() {
        assert_eq!(default_config().max_tokens, Some(DEFAULT_MAX_TOKENS));
    }

    #[test]
    fn default_thinking_is_false() {
        assert!(!default_config().thinking);
    }

    #[test]
    fn default_include_stream_usage_is_true() {
        assert!(default_config().include_stream_usage);
    }

    // ── base_url resolution ───────────────────────────────────────────────────

    #[test]
    fn base_url_returns_anthropic_default_when_no_override() {
        let config = AppConfig {
            provider: Provider::Anthropic,
            ..AppConfig::default()
        };
        assert_eq!(config.base_url(), DEFAULT_ANTHROPIC_BASE_URL);
    }

    #[test]
    fn base_url_returns_openai_default_for_openai_provider() {
        let config = AppConfig {
            provider: Provider::OpenAi,
            ..AppConfig::default()
        };
        assert_eq!(config.base_url(), DEFAULT_OPENAI_BASE_URL);
    }

    #[test]
    fn base_url_returns_custom_override_regardless_of_provider() {
        let config = AppConfig {
            provider: Provider::Anthropic,
            api_url: Some("https://my-proxy.example.com".to_string()),
            ..AppConfig::default()
        };
        assert_eq!(config.base_url(), "https://my-proxy.example.com");
    }

    // ── user_agent ───────────────────────────────────────────────────────────

    #[test]
    fn user_agent_falls_back_to_default() {
        let config = AppConfig {
            user_agent: None,
            ..AppConfig::default()
        };
        assert_eq!(config.user_agent(), DEFAULT_USER_AGENT);
    }

    #[test]
    fn user_agent_returns_custom_value() {
        let config = AppConfig {
            user_agent: Some("my-bot/2.0".to_string()),
            ..AppConfig::default()
        };
        assert_eq!(config.user_agent(), "my-bot/2.0");
    }

    // ── Debug redaction ───────────────────────────────────────────────────────

    #[test]
    fn debug_output_redacts_api_key() {
        let config = AppConfig {
            api_key: Some("sk-secret-key-1234".to_string()),
            ..AppConfig::default()
        };
        let debug_str = format!("{:?}", config);
        assert!(
            !debug_str.contains("sk-secret-key-1234"),
            "API key must not appear in Debug output"
        );
        assert!(
            debug_str.contains("[REDACTED]"),
            "Debug output should contain [REDACTED] placeholder"
        );
    }

    #[test]
    fn debug_output_shows_none_for_missing_key() {
        let config = AppConfig {
            api_key: None,
            ..AppConfig::default()
        };
        let debug_str = format!("{:?}", config);
        // None api_key → map(|_| "[REDACTED]") → None, shown as "None"
        assert!(debug_str.contains("api_key: None"));
    }
}
