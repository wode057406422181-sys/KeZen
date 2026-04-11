use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::{Provider, default_true};

use crate::constants::api::{DEFAULT_MAX_TOKENS, DEFAULT_USER_AGENT};

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
    /// Max output tokens for this model. Defaults to 128_000.
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    /// Context window size for this model (tokens). Defaults to 200_000.
    #[serde(default = "default_context_window")]
    pub context_window: u64,
    /// HTTP User-Agent string. Defaults to "kezen/<version>".
    #[serde(default = "default_user_agent")]
    pub user_agent: String,
}

fn default_max_tokens() -> u32 {
    DEFAULT_MAX_TOKENS
}

fn default_context_window() -> u64 {
    200_000
}

fn default_user_agent() -> String {
    DEFAULT_USER_AGENT.to_string()
}

impl Default for ModelProfile {
    fn default() -> Self {
        Self {
            provider: Provider::default(),
            model: String::new(),
            api_key: None,
            api_url: None,
            thinking: false,
            include_stream_usage: true,
            enable_cache: true,
            max_tokens: DEFAULT_MAX_TOKENS,
            context_window: 200_000,
            user_agent: DEFAULT_USER_AGENT.to_string(),
        }
    }
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

/// Returns the context window size for a given model.
pub fn context_window_for_model(model: &str) -> u64 {
    if model.contains("opus") || model.contains("sonnet") || model.contains("haiku") {
        200_000
    } else if model.contains("gpt-4o") {
        128_000
    } else if model.contains("gemini") && model.contains("pro") {
        1_000_000
    } else {
        128_000 // Default safe value
    }
}

impl crate::config::AppConfig {
    /// Get the base URL for the configured provider
    pub fn base_url(&self) -> &str {
        self.api_url.as_deref().unwrap_or(match self.provider {
            Provider::Anthropic => crate::constants::api::DEFAULT_ANTHROPIC_BASE_URL,
            Provider::OpenAi => crate::constants::api::DEFAULT_OPENAI_BASE_URL,
        })
    }

    /// Get the active model profile (if resolved).
    pub fn active_model_profile(&self) -> Option<&ModelProfile> {
        self.active_profile
            .as_ref()
            .and_then(|k| self.models.get(k))
    }

    /// Get max output tokens from the active model profile, or the built-in default.
    pub fn max_tokens(&self) -> u32 {
        self.active_model_profile()
            .map(|p| p.max_tokens)
            .unwrap_or(DEFAULT_MAX_TOKENS)
    }

    /// Get context window from the active model profile.
    pub fn context_window(&self) -> u64 {
        self.active_model_profile()
            .map(|p| p.context_window)
            .unwrap_or_else(|| {
                if let Some(m) = &self.model {
                    context_window_for_model(m)
                } else {
                    200_000
                }
            })
    }

    /// Get User-Agent string from the active model profile, or the built-in default.
    pub fn user_agent(&self) -> &str {
        self.active_model_profile()
            .map(|p| p.user_agent.as_str())
            .unwrap_or(DEFAULT_USER_AGENT)
    }

    /// Resolves a model name against the predefined model profiles (`[models]`).
    /// Updates the configuration (provider, model, API keys/URLs) if matched.
    pub fn resolve_model_profile(&mut self, profile_name: &str) {
        if let Some(profile) = self.models.get(profile_name).cloned() {
            self.active_profile = Some(profile_name.to_string());
            self.provider = profile.provider;
            self.model = Some(profile.model);
            self.thinking = profile.thinking;
            self.include_stream_usage = profile.include_stream_usage;
            self.enable_cache = profile.enable_cache;
            if let Some(key) = profile.api_key {
                self.api_key = crate::config::keys::resolve_key(Some(key));
            }
            if let Some(url) = profile.api_url {
                self.api_url = Some(url);
            }
        } else {
            self.model = Some(profile_name.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_window_unknown_model_defaults() {
        assert_eq!(context_window_for_model("llama-3.1-70b"), 128_000);
    }
}
