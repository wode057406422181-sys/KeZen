use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

use anyhow::Result;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};

use super::default_true;

/// Serde helpers for `Option<SecretString>`: deserialize from plain string,
/// serialize by exposing the secret (needed for `model.toml` round-tripping).
mod secret_string_serde {
    use secrecy::{ExposeSecret, SecretString};
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &Option<SecretString>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(s) => serializer.serialize_some(s.expose_secret()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<SecretString>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt: Option<String> = Option::deserialize(deserializer)?;
        Ok(opt.map(SecretString::from))
    }
}

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

use crate::constants::api::{DEFAULT_MAX_TOKENS, DEFAULT_USER_AGENT};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelProfile {
    #[serde(default)]
    pub provider: Provider,
    pub model: String,
    #[serde(default, with = "secret_string_serde")]
    pub api_key: Option<SecretString>,
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
    /// Get the effective provider from the runtime profile.
    pub fn provider(&self) -> Provider {
        self.runtime_profile.provider
    }

    /// Get whether extended thinking is enabled.
    pub fn thinking(&self) -> bool {
        self.runtime_profile.thinking
    }

    /// Get whether stream usage reporting is enabled (OpenAI).
    pub fn include_stream_usage(&self) -> bool {
        self.runtime_profile.include_stream_usage
    }

    /// Get whether prompt caching is enabled.
    pub fn enable_cache(&self) -> bool {
        self.runtime_profile.enable_cache
    }

    /// Get the base URL for the configured provider.
    pub fn base_url(&self) -> &str {
        self.runtime_profile
            .api_url
            .as_deref()
            .unwrap_or(match self.runtime_profile.provider {
                Provider::Anthropic => crate::constants::api::DEFAULT_ANTHROPIC_BASE_URL,
                Provider::OpenAi => crate::constants::api::DEFAULT_OPENAI_BASE_URL,
            })
    }

    /// Get max output tokens from the runtime profile.
    pub fn max_tokens(&self) -> u32 {
        self.runtime_profile.max_tokens
    }

    /// Get context window from the runtime profile.
    pub fn context_window(&self) -> u64 {
        self.runtime_profile.context_window
    }

    /// Get User-Agent string from the runtime profile.
    pub fn user_agent(&self) -> &str {
        &self.runtime_profile.user_agent
    }

    /// Get the resolved API key from the runtime profile.
    pub fn api_key(&self) -> Option<&SecretString> {
        self.runtime_profile.api_key.as_ref()
    }

    /// Resolves a model name against the predefined model profiles (`[models]`).
    ///
    /// Copies the matched profile into `runtime_profile`, preserving any
    /// previously-set values (e.g. `api_url` from ENV) when the profile
    /// field is `None`.
    pub fn resolve_model_profile(&mut self, profile_name: &str) {
        if let Some(profile) = self.models.get(profile_name).cloned() {
            self.model = Some(profile.model.clone());

            // Preserve ENV-set api_url/api_key if profile doesn't specify one.
            let prev_api_url = self.runtime_profile.api_url.take();
            let prev_api_key = self.runtime_profile.api_key.take();
            self.runtime_profile = profile;
            if self.runtime_profile.api_url.is_none() {
                self.runtime_profile.api_url = prev_api_url;
            }
            if self.runtime_profile.api_key.is_none() {
                self.runtime_profile.api_key = prev_api_key;
            }

            // Resolve keystore:// references in-place.
            if let Some(ref key) = self.runtime_profile.api_key {
                if key.expose_secret().starts_with("keystore://") {
                    self.runtime_profile.api_key =
                        crate::config::keys::resolve_key(Some(key.expose_secret().to_string()));
                }
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
