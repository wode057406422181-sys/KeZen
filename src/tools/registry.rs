use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use super::Tool;
use crate::config::AppConfig;

/// Central registry mapping tool names to their implementations.
///
/// The engine uses this to look up tools requested by the LLM and to
/// generate the combined JSON schema array sent with each API call.
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
    /// Alias → canonical name mapping (e.g. "Read" → "FileRead").
    /// When `get()` misses on the primary map it falls back here.
    aliases: HashMap<String, String>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            aliases: HashMap::new(),
        }
    }

    /// Add a tool to the registry, keyed by its `name()`.
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Register an alias that maps to a canonical tool name.
    /// When `get(alias)` is called, it transparently returns the canonical tool.
    pub fn register_alias(&mut self, alias: impl Into<String>, canonical: impl Into<String>) {
        self.aliases.insert(alias.into(), canonical.into());
    }

    /// Look up a tool by name. Falls back to alias mapping if the exact name
    /// is not found. Returns `None` only if neither matches.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned().or_else(|| {
            self.aliases.get(name).and_then(|canonical| self.tools.get(canonical).cloned())
        })
    }

    /// Generate the JSON tool schemas array for the LLM API request.
    pub fn schemas(&self) -> Vec<serde_json::Value> {
        self.tools.values().map(|t| {
            serde_json::json!({
                "name": t.name(),
                "description": t.description(),
                "input_schema": t.input_schema()
            })
        }).collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}


/// Create a registry pre-loaded with all built-in tools.
///
/// `config` controls which optional tools are registered:
///
/// Web tool registration rules:
///   - `search_mode = "off" | "native"` (or no config) → skip WebSearchTool
///   - `search_mode = "brave"|…`                      → register WebSearchTool
///   - `fetch_mode  = "native"`                        → skip WebFetchTool
///   - `fetch_mode  = "client"` (or no config)         → register WebFetchTool
pub fn create_default_registry(config: &AppConfig, work_dir: PathBuf) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(super::bash::BashTool));
    registry.register(Arc::new(super::file_read::FileReadTool));
    registry.register(Arc::new(super::file_write::FileWriteTool { work_dir: work_dir.clone() }));
    registry.register(Arc::new(super::file_edit::FileEditTool { work_dir: work_dir.clone() }));
    registry.register(Arc::new(super::grep::GrepTool { work_dir: work_dir.clone() }));
    registry.register(Arc::new(super::glob::GlobTool { work_dir }));

    // Resolve effective modes.
    // No [search] section: search_mode defaults to "off", fetch_mode defaults to "client".
    let search_mode = config.search.as_ref()
        .map(|s| s.search_mode.as_str())
        .unwrap_or("off");
    let fetch_mode = config.search.as_ref()
        .map(|s| s.fetch_mode.as_str())
        .unwrap_or("client");

    // WebSearchTool: only for explicit client-side backends (not "off" or "native").
    // In native mode, Dashscope handles search internally, so we MUST hide the tool schema.
    if search_mode != "off" && search_mode != "native" {
        registry.register(Arc::new(
            super::web_search::WebSearchTool::new(config.search.clone()),
        ));
    }

    // WebFetchTool: registered by default ("client"), skipped only for "native".
    if fetch_mode != "native" {
        registry.register(Arc::new(
            super::web_fetch::WebFetchTool::new(Some(config.clone())),
        ));
    }

    // Register common aliases so that models emitting "Read" / "Write"
    // instead of "FileRead" / "FileWrite" still resolve correctly.
    registry.register_alias("Read", "FileRead");
    registry.register_alias("Write", "FileWrite");

    registry
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SearchConfig;

    #[test]
    fn test_registry_register_and_get() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(crate::tools::bash::BashTool));
        assert!(registry.get("Bash").is_some());
        assert!(registry.get("Unknown").is_none());
    }

    #[test]
    fn test_registry_schemas() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(crate::tools::bash::BashTool));
        let schemas = registry.schemas();
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0]["name"], "Bash");
    }

    // No [search] section → search off, fetch defaults to client → only WebFetchTool
    #[test]
    fn test_no_search_config_defaults() {
        let config = AppConfig::default();
        let registry = create_default_registry(&config, std::env::current_dir().unwrap());
        assert!(registry.get("Bash").is_some());
        assert!(registry.get("WebSearch").is_none());
        assert!(registry.get("WebFetch").is_some()); // default fetch_mode = "client"
        assert_eq!(registry.schemas().len(), 7);
    }

    // Explicit SearchConfig with defaults → search off, fetch client
    #[test]
    fn test_explicit_default_search_config() {
        let mut config = AppConfig::default();
        config.search = Some(SearchConfig::default());
        let registry = create_default_registry(&config, std::env::current_dir().unwrap());
        assert!(registry.get("WebSearch").is_none());
        assert!(registry.get("WebFetch").is_some());
        assert_eq!(registry.schemas().len(), 7);
    }

    // Client search (brave) + native fetch → only WebSearchTool
    #[test]
    fn test_client_search_native_fetch() {
        let mut config = AppConfig::default();
        config.search = Some(SearchConfig {
            search_mode: "brave".into(),
            fetch_mode: "native".into(),
            api_key: Some("key".into()),
            ..SearchConfig::default()
        });
        let registry = create_default_registry(&config, std::env::current_dir().unwrap());
        assert!(registry.get("WebSearch").is_some());
        assert!(registry.get("WebFetch").is_none());
        assert_eq!(registry.schemas().len(), 7);
    }

    // Native search + client fetch → only WebFetchTool
    #[test]
    fn test_native_search_client_fetch() {
        let mut config = AppConfig::default();
        config.search = Some(SearchConfig {
            search_mode: "native".into(),
            fetch_mode: "client".into(),
            ..SearchConfig::default()
        });
        let registry = create_default_registry(&config, std::env::current_dir().unwrap());
        assert!(registry.get("WebSearch").is_none());
        assert!(registry.get("WebFetch").is_some());
        assert_eq!(registry.schemas().len(), 7);
    }

    // Explicit native fetch → WebFetchTool NOT registered
    #[test]
    fn test_explicit_native_fetch() {
        let mut config = AppConfig::default();
        config.search = Some(SearchConfig {
            search_mode: "off".into(),
            fetch_mode: "native".into(),
            ..SearchConfig::default()
        });
        let registry = create_default_registry(&config, std::env::current_dir().unwrap());
        assert!(registry.get("WebSearch").is_none());
        assert!(registry.get("WebFetch").is_none());
        assert_eq!(registry.schemas().len(), 6);
    }

    // Both client → both tools
    #[test]
    fn test_client_search_client_fetch() {
        let mut config = AppConfig::default();
        config.search = Some(SearchConfig {
            search_mode: "brave".into(),
            fetch_mode: "client".into(),
            api_key: Some("key".into()),
            ..SearchConfig::default()
        });
        let registry = create_default_registry(&config, std::env::current_dir().unwrap());
        assert!(registry.get("WebSearch").is_some());
        assert!(registry.get("WebFetch").is_some());
        assert_eq!(registry.schemas().len(), 8);
    }

    // SearXNG search + client fetch
    #[test]
    fn test_searxng_search_client_fetch() {
        let mut config = AppConfig::default();
        config.search = Some(SearchConfig {
            search_mode: "searxng".into(),
            fetch_mode: "client".into(),
            base_url: Some("http://localhost:8080".into()),
            ..SearchConfig::default()
        });
        let registry = create_default_registry(&config, std::env::current_dir().unwrap());
        assert!(registry.get("WebSearch").is_some());
        assert!(registry.get("WebFetch").is_some());
        assert_eq!(registry.schemas().len(), 8);
    }

    // Native + search_strategy does not affect tool registration
    #[test]
    fn test_native_with_search_strategy() {
        let mut config = AppConfig::default();
        config.search = Some(SearchConfig {
            search_mode: "native".into(), // native = skip WebSearchTool
            search_strategy: Some("agent_max".into()),
            ..SearchConfig::default()
        });
        let registry = create_default_registry(&config, std::env::current_dir().unwrap());
        assert!(registry.get("WebSearch").is_none());
        assert!(registry.get("WebFetch").is_some()); // default fetch_mode = "client"
        assert_eq!(registry.schemas().len(), 7);
    }

    #[test]
    fn test_registry_default_impl() {
        let registry = ToolRegistry::default();
        assert!(registry.schemas().is_empty());
    }

    #[test]
    fn test_registry_overwrite_same_name() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(crate::tools::bash::BashTool));
        registry.register(Arc::new(crate::tools::bash::BashTool));
        assert_eq!(registry.schemas().len(), 1);
    }

    // ── Alias tests ──────────────────────────────────────────────────

    #[test]
    fn test_alias_resolves_to_canonical() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(crate::tools::file_read::FileReadTool));
        registry.register_alias("Read", "FileRead");
        assert!(registry.get("Read").is_some());
        // The underlying tool should be the same
        assert_eq!(registry.get("Read").unwrap().name(), "FileRead");
    }

    #[test]
    fn test_alias_returns_none_for_missing_canonical() {
        let mut registry = ToolRegistry::new();
        registry.register_alias("Read", "FileRead");
        // No tool "FileRead" registered, alias should return None
        assert!(registry.get("Read").is_none());
    }

    #[test]
    fn test_exact_name_takes_precedence_over_alias() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(crate::tools::bash::BashTool));
        // Even if there's an alias named "Bash", exact match wins
        registry.register_alias("Bash", "FileRead");
        assert_eq!(registry.get("Bash").unwrap().name(), "Bash");
    }

    #[test]
    fn test_default_registry_aliases() {
        let config = AppConfig::default();
        let registry = create_default_registry(&config, std::env::current_dir().unwrap());
        // Read → FileRead
        assert!(registry.get("Read").is_some());
        assert_eq!(registry.get("Read").unwrap().name(), "FileRead");
        // Write → FileWrite
        assert!(registry.get("Write").is_some());
        assert_eq!(registry.get("Write").unwrap().name(), "FileWrite");
    }

    #[test]
    fn test_schemas_contain_required_fields() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(crate::tools::bash::BashTool));
        let s = &registry.schemas()[0];
        assert!(s.get("name").is_some());
        assert!(s.get("description").is_some());
        assert!(s.get("input_schema").is_some());
    }
}
