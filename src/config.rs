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

/// Configuration for web search and fetch capabilities.
///
/// Defaults when no `[search]` section is present (or fields are omitted):
///   - `search_mode = "off"` — no search at all (neither native nor client-side).
///   - `fetch_mode  = "client"` — WebFetchTool is always registered.
///
/// `search_mode` values:
///   - `"off"`: No web search (default).
///   - `"native"`: Server-side search via provider API (DashScope `enable_search`, etc.).
///   - `"brave"`, `"searxng"`, `"google_cse"`, `"bing"`: Client-side search.
///
/// `fetch_mode` values:
///   - `"client"`: Client-side WebFetchTool (HTML→Markdown + optional LLM extraction) (default).
///   - `"native"`: Server-side fetch via provider API.
///
/// Set via `[search]` section in `~/.kezen/config/config.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfig {
    /// Web search mode. Default: `"off"`.
    #[serde(default = "default_search_mode")]
    pub search_mode: String,
    /// Web fetch mode. Default: `"client"`.
    #[serde(default = "default_fetch_mode")]
    pub fetch_mode: String,
    /// API key for the search provider (not needed for `native` mode).
    pub api_key: Option<String>,
    /// Base URL (e.g. SearXNG instance URL, or Google CSE CX id).
    pub base_url: Option<String>,
    /// Search strategy hint for native mode (DashScope: turbo/max/agent/agent_max).
    /// Defaults to `"turbo"` when omitted.
    pub search_strategy: Option<String>,
}

fn default_search_mode() -> String {
    "off".to_string()
}

fn default_fetch_mode() -> String {
    "client".to_string()
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            search_mode: default_search_mode(),
            fetch_mode: default_fetch_mode(),
            api_key: None,
            base_url: None,
            search_strategy: None,
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

    /// Override the context window size
    pub context_window: Option<u64>,

    /// Enable extended thinking (Anthropic only)
    #[serde(default)]
    pub thinking: bool,

    /// Custom User-Agent header (useful for Coding Plan endpoints)
    pub user_agent: Option<String>,

    /// Disable MCP server connections
    #[serde(default)]
    pub no_mcp: bool,

    /// Send stream_options.include_usage in OpenAI streaming requests.
    ///
    /// Set to `false` for endpoints that don't support this field (DashScope,
    /// Ollama, vLLM, etc.). Defaults to `true` for the official OpenAI API.
    #[serde(default = "default_true")]
    pub include_stream_usage: bool,

    /// Web search configuration (loaded from [search] section).
    pub search: Option<SearchConfig>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            provider: Provider::Anthropic,
            api_url: None,
            api_key: None,
            model: None,
            max_tokens: Some(DEFAULT_MAX_TOKENS),
            context_window: None,
            thinking: false,
            user_agent: None,
            no_mcp: false,
            include_stream_usage: true,
            search: None,
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
    ///
    /// # Blocking I/O
    ///
    /// This method uses `std::fs::read_to_string` and `path.exists()`
    /// (synchronous / blocking I/O) **intentionally**.  It is called
    /// from the synchronous `main()` entry point *before* the tokio
    /// runtime is started, so there is no async runtime to block.
    ///
    /// **Do NOT call this from an async context.**  If you need to
    /// reload config at runtime (e.g. from `/model`), create a
    /// separate async variant using `tokio::fs`.
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
                    config.max_tokens = Some(DEFAULT_MAX_TOKENS);
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

        // Search-specific env overrides
        if let Ok(val) = std::env::var("KEZEN_SEARCH_MODE") {
            config.search.get_or_insert_with(SearchConfig::default).search_mode = val;
        }
        if let Ok(val) = std::env::var("KEZEN_FETCH_MODE") {
            config.search.get_or_insert_with(SearchConfig::default).fetch_mode = val;
        }
        if let Ok(val) = std::env::var("KEZEN_SEARCH_API_KEY") {
            config.search.get_or_insert_with(SearchConfig::default).api_key = Some(val);
        }
        if let Ok(val) = std::env::var("KEZEN_SEARCH_STRATEGY") {
            config.search.get_or_insert_with(SearchConfig::default).search_strategy = Some(val);
        }

        Ok(config)
    }

    /// Save configuration to the default config file.
    ///
    /// # Blocking I/O
    ///
    /// Uses synchronous `std::fs::write`.  Only call from a synchronous
    /// context or from `tokio::task::spawn_blocking`.  See [`load()`]
    /// for rationale.
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
            .field("search", &self.search)
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
    fn default_max_tokens_is_correct() {
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

    // ── SearchConfig ─────────────────────────────────────────────────────────

    #[test]
    fn default_search_is_none() {
        assert!(default_config().search.is_none());
    }

    #[test]
    fn search_config_default_is_off_client() {
        let sc = SearchConfig::default();
        assert_eq!(sc.search_mode, "off");
        assert_eq!(sc.fetch_mode, "client");
        assert!(sc.api_key.is_none());
        assert!(sc.search_strategy.is_none());
    }

    #[test]
    fn search_config_deserializes_defaults_when_empty() {
        let toml_str = r#"
            api_key = "test"
        "#;
        let sc: SearchConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(sc.search_mode, "off");     // default
        assert_eq!(sc.fetch_mode, "client");   // default
    }

    #[test]
    fn search_config_deserializes_native_search_with_strategy() {
        let toml_str = r#"
            search_mode = "native"
            search_strategy = "agent_max"
        "#;
        let sc: SearchConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(sc.search_mode, "native");
        assert_eq!(sc.fetch_mode, "client"); // default
        assert_eq!(sc.search_strategy.as_deref(), Some("agent_max"));
    }

    #[test]
    fn search_config_deserializes_brave_with_client_fetch() {
        let toml_str = r#"
            search_mode = "brave"
            fetch_mode = "client"
            api_key = "BSA-test-key"
        "#;
        let sc: SearchConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(sc.search_mode, "brave");
        assert_eq!(sc.fetch_mode, "client");
        assert_eq!(sc.api_key.as_deref(), Some("BSA-test-key"));
    }

    #[test]
    fn search_config_independent_modes() {
        let toml_str = r#"
            search_mode = "brave"
            fetch_mode = "native"
            api_key = "key"
        "#;
        let sc: SearchConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(sc.search_mode, "brave");
        assert_eq!(sc.fetch_mode, "native");
    }

    #[test]
    fn app_config_with_search_section() {
        let toml_str = r#"
            provider = "openai"
            [search]
            search_mode = "native"
            fetch_mode = "client"
            search_strategy = "turbo"
        "#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert!(config.search.is_some());
        let search = config.search.unwrap();
        assert_eq!(search.search_mode, "native");
        assert_eq!(search.fetch_mode, "client");
        assert_eq!(search.search_strategy.as_deref(), Some("turbo"));
    }

    #[test]
    fn app_config_without_search_section() {
        let toml_str = r#"
            provider = "anthropic"
        "#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert!(config.search.is_none());
    }

    #[test]
    fn app_config_empty_search_section_defaults_to_off_client() {
        let toml_str = r#"
            provider = "openai"
            [search]
        "#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();
        let search = config.search.unwrap();
        assert_eq!(search.search_mode, "off");
        assert_eq!(search.fetch_mode, "client");
    }
}
