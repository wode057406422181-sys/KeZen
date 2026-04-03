use std::collections::HashMap;

use super::Tool;

pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

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

pub fn create_default_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(super::bash::BashTool));
    registry.register(Box::new(super::file_read::FileReadTool));
    registry.register(Box::new(super::file_write::FileWriteTool));
    registry.register(Box::new(super::file_edit::FileEditTool));
    registry.register(Box::new(super::grep::GrepTool));
    registry.register(Box::new(super::glob::GlobTool));
    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_register_and_get() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(crate::tools::bash::BashTool));
        assert!(registry.get("Bash").is_some());
        assert!(registry.get("Unknown").is_none());
    }

    #[test]
    fn test_registry_schemas() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(crate::tools::bash::BashTool));
        let schemas = registry.schemas();
        assert_eq!(schemas.len(), 1);
        let schema = &schemas[0];
        assert_eq!(schema["name"], "Bash");
    }

    #[test]
    fn test_create_default_registry() {
        let registry = create_default_registry();
        assert!(registry.get("Bash").is_some());
        assert!(registry.get("FileRead").is_some());
        assert!(registry.get("FileWrite").is_some());
        assert!(registry.get("FileEdit").is_some());
        assert!(registry.get("Grep").is_some());
        assert!(registry.get("Glob").is_some());
        assert_eq!(registry.schemas().len(), 6);
    }
}
