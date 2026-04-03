use async_trait::async_trait;
use serde_json::json;
use glob::glob;

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
            Some(p) => p,
            None => {
                return ToolResult {
                    content: "Error: missing or invalid 'pattern'".to_string(),
                    is_error: true,
                }
            }
        };

        let current_dir = std::env::current_dir().unwrap_or_default();
        let path_str = input.get("path").and_then(|v| v.as_str()).unwrap_or(current_dir.to_str().unwrap_or("."));
        
        let search_pattern = format!("{}/{}", path_str, pattern);
        
        let mut results = Vec::new();
        let mut num_files = 0;

        match glob(&search_pattern) {
            Ok(paths) => {
                for entry in paths.filter_map(Result::ok) {
                    let path_lossy = entry.to_string_lossy();
                    if path_lossy.contains("/.git/") || path_lossy.contains("/node_modules/") {
                        continue;
                    }

                    results.push(path_lossy.into_owned());
                    num_files += 1;

                    if num_files >= 100 {
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

        let mut content = format!("Found {} file{}:\n", num_files, if num_files == 1 { "" } else { "s" });
        content.push_str(&results.join("\n"));

        if num_files >= 100 {
            content.push_str("\n(Results are truncated. Consider using a more specific path or pattern.)");
        }

        ToolResult {
            content,
            is_error: false,
        }
    }
}
