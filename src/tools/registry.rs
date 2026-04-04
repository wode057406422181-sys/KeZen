use std::collections::HashMap;
use std::sync::Arc;

use super::Tool;
use crate::config::AppConfig;

/// Central registry mapping tool names to their implementations.
///
/// The engine uses this to look up tools requested by the LLM and to
/// generate the combined JSON schema array sent with each API call.
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Add a tool to the registry, keyed by its `name()`.
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Look up a tool by name. Returns `None` if not registered.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
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
/// `config` is needed for tools that require API keys or provider
/// settings (e.g. WebSearchTool needs SearchConfig, WebFetchTool needs
/// the full AppConfig for sub-LLM content extraction).
pub fn create_default_registry(config: &AppConfig) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(super::bash::BashTool));
    registry.register(Arc::new(super::file_read::FileReadTool));
    registry.register(Arc::new(super::file_write::FileWriteTool));
    registry.register(Arc::new(super::file_edit::FileEditTool));
    registry.register(Arc::new(super::grep::GrepTool));
    registry.register(Arc::new(super::glob::GlobTool));
    // Web tools: conditional registration based on search mode.
    //
    // • mode = "native"  → Server-side search & fetch; don't register client-side tools.
    // • mode = "brave"/… → Client-side search + generic fetch.
    // • No search config → Only generic WebFetchTool (always useful for URL retrieval).
    match config.search.as_ref().map(|s| s.mode.as_str()) {
        Some("native") => {
            // Server-side search & fetch handled by the API layer.
            // No client-side tools needed — saves API schema tokens.
        }
        Some(_) => {
            // Client-side search backend configured.
            registry.register(Arc::new(super::web_search::WebSearchTool::new(config.search.clone())));
            registry.register(Arc::new(super::web_fetch::WebFetchTool::new(Some(config.clone()))));
        }
        None => {
            // No search config — still register WebFetch for generic URL retrieval.
            registry.register(Arc::new(super::web_fetch::WebFetchTool::new(Some(config.clone()))));
        }
    }
    registry
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let schema = &schemas[0];
        assert_eq!(schema["name"], "Bash");
    }

    #[test]
    fn test_create_default_registry_without_search() {
        let config = AppConfig::default();
        let registry = create_default_registry(&config);
        assert!(registry.get("Bash").is_some());
        assert!(registry.get("FileRead").is_some());
        assert!(registry.get("FileWrite").is_some());
        assert!(registry.get("FileEdit").is_some());
        assert!(registry.get("Grep").is_some());
        assert!(registry.get("Glob").is_some());
        assert!(registry.get("WebSearch").is_none()); // Not registered without config
        assert!(registry.get("WebFetch").is_some());
        assert_eq!(registry.schemas().len(), 7);
    }

    #[test]
    fn test_create_default_registry_with_search() {
        let mut config = AppConfig::default();
        config.search = Some(crate::config::SearchConfig {
            mode: "brave".into(),
            api_key: Some("test-key".into()),
            base_url: None,
            search_strategy: None,
        });
        let registry = create_default_registry(&config);
        assert!(registry.get("WebSearch").is_some());
        assert!(registry.get("WebFetch").is_some());
        assert_eq!(registry.schemas().len(), 8);
    }

    #[test]
    fn test_create_default_registry_native_mode_no_web_tools() {
        let mut config = AppConfig::default();
        config.search = Some(crate::config::SearchConfig {
            mode: "native".into(),
            api_key: None,
            base_url: None,
            search_strategy: Some("turbo".into()),
        });
        let registry = create_default_registry(&config);
        // Native mode: server-side handles search & fetch, no client tools registered.
        assert!(registry.get("WebSearch").is_none());
        assert!(registry.get("WebFetch").is_none());
        assert_eq!(registry.schemas().len(), 6); // Only core tools
    }
}

