use async_trait::async_trait;
use serde_json::json;

use super::{Tool, ToolResult};

pub struct GlobTool;

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "Glob"
    }

    fn description(&self) -> &str {
        "Find files by name pattern or wildcard"
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The glob pattern to match files against"
                },
                "path": {
                    "type": "string",
                    "description": "The directory to search in. Defaults to current directory"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn call(&self, input: serde_json::Value) -> ToolResult {
        let pattern = match input.get("pattern").and_then(|v| v.as_str()) {
            Some(p) => p.to_string(),
            None => {
                return ToolResult {
                    content: "Error: missing or invalid 'pattern'".to_string(),
                    is_error: true,
                }
            }
        };

        let current_dir = std::env::current_dir().unwrap_or_default();
        let path_str = input.get("path").and_then(|v| v.as_str())
            .unwrap_or(current_dir.to_str().unwrap_or("."))
            .to_string();

        // Offload directory traversal to a blocking thread
        let result = tokio::task::spawn_blocking(move || {
            let search_pattern = format!("{}/{}", path_str, pattern);

            let mut results = Vec::new();

            match glob::glob(&search_pattern) {
                Ok(paths) => {
                    for entry in paths.filter_map(Result::ok) {
                        let path_lossy = entry.to_string_lossy().to_string();
                        if path_lossy.contains("/.git/") || path_lossy.contains("/node_modules/") {
                            continue;
                        }

                        results.push(path_lossy);

                        if results.len() >= 100 {
                            break;
                        }
                    }
                }
                Err(e) => {
                    return ToolResult {
                        content: format!("Invalid glob pattern: {}", e),
                        is_error: true,
                    };
                }
            }

            if results.is_empty() {
                return ToolResult {
                    content: "No files found".to_string(),
                    is_error: false,
                };
            }

            let count = results.len();
            let mut content = format!("Found {} file{}:\n", count, if count == 1 { "" } else { "s" });
            content.push_str(&results.join("\n"));

            if count >= 100 {
                content.push_str("\n(Results are truncated. Consider using a more specific path or pattern.)");
            }

            ToolResult {
                content,
                is_error: false,
            }
        }).await;

        match result {
            Ok(r) => r,
            Err(e) => ToolResult {
                content: format!("Glob task panicked: {}", e),
                is_error: true,
            },
        }
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_glob_finds_files() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn main(){}").unwrap();
        std::fs::write(dir.path().join("b.txt"), "hello").unwrap();
        
        let tool = GlobTool;
        let result = tool.call(json!({
            "pattern": "*.rs",
            "path": dir.path().to_str().unwrap()
        })).await;
        
        assert!(!result.is_error);
        assert!(result.content.contains("a.rs"));
        assert!(!result.content.contains("b.txt"));
    }

    #[tokio::test]
    async fn test_glob_no_match() {
        let dir = tempdir().unwrap();
        
        let tool = GlobTool;
        let result = tool.call(json!({
            "pattern": "*.rs",
            "path": dir.path().to_str().unwrap()
        })).await;
        
        assert!(!result.is_error);
        assert!(result.content.contains("No files found"));
    }

    #[tokio::test]
    async fn test_glob_invalid_pattern() {
        let tool = GlobTool;
        let _result = tool.call(json!({
            "pattern": "***"
        })).await;
        
        // glob crate allows `***` but some invalid pattern like `[` without `]` will err
        let result2 = tool.call(json!({
            "pattern": "["
        })).await;
        assert!(result2.is_error);
        assert!(result2.content.contains("Invalid glob pattern"));
    }
}
