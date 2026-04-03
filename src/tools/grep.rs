use async_trait::async_trait;
use serde_json::json;
use regex::RegexBuilder;
use glob::glob;
use std::fs;

use super::{Tool, ToolResult};

pub struct GrepTool;

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "Grep"
    }

    fn description(&self) -> &str {
        "Search file contents with regex pattern"
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The regular expression pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "Directory to search in"
                },
                "include": {
                    "type": "string",
                    "description": "Glob pattern to filter files (e.g. *.rs)"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn call(&self, input: serde_json::Value) -> ToolResult {
        let pattern_str = match input.get("pattern").and_then(|v| v.as_str()) {
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
        let include_pat = input.get("include").and_then(|v| v.as_str()).unwrap_or("**/*");

        let search_pattern = format!("{}/{}", path_str, include_pat);
        
        let re = match RegexBuilder::new(pattern_str).build() {
            Ok(r) => r,
            Err(e) => {
                return ToolResult {
                    content: format!("Invalid Regex pattern: {}", e),
                    is_error: true,
                };
            }
        };

        let mut results = String::new();
        let mut match_count = 0;

        if let Ok(paths) = glob(&search_pattern) {
            for entry in paths.filter_map(Result::ok) {
                let p = entry.to_string_lossy();
                if p.contains("/.git/") || p.contains("/node_modules/") || p.contains("/target/") {
                    continue;
                }

                if !entry.is_file() {
                    continue;
                }

                if let Ok(content) = fs::read_to_string(&entry) {
                    for (line_num, line) in content.lines().enumerate() {
                        if re.is_match(line) {
                            results.push_str(&format!("{}:{}: {}\n", p, line_num + 1, line));
                            match_count += 1;
                            if match_count >= 50 {
                                break;
                            }
                        }
                    }
                }

                if match_count >= 50 {
                    break;
                }
            }
        }

        if match_count == 0 {
            return ToolResult {
                content: "No matches found".to_string(),
                is_error: false,
            };
        }

        if match_count >= 50 {
            results.push_str("\n[Showing results with pagination = limit: 50]");
        }

        ToolResult {
            content: results,
            is_error: false,
        }
    }
}
