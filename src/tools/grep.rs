use async_trait::async_trait;
use serde_json::json;
use regex::RegexBuilder;

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
        let include_pat = input.get("include").and_then(|v| v.as_str())
            .unwrap_or("**/*")
            .to_string();

        // Offload CPU-intensive glob traversal + regex matching to a blocking thread
        let result = tokio::task::spawn_blocking(move || {
            let search_pattern = format!("{}/{}", path_str, include_pat);

            let re = match RegexBuilder::new(&pattern_str).build() {
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

            if let Ok(paths) = glob::glob(&search_pattern) {
                for entry in paths.filter_map(Result::ok) {
                    let p = entry.to_string_lossy().to_string();
                    if p.contains("/.git/") || p.contains("/node_modules/") || p.contains("/target/") {
                        continue;
                    }

                    if !entry.is_file() {
                        continue;
                    }

                    if let Ok(content) = std::fs::read_to_string(&entry) {
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
        }).await;

        match result {
            Ok(r) => r,
            Err(e) => ToolResult {
                content: format!("Grep task panicked: {}", e),
                is_error: true,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_grep_finds_matches() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("match.txt");
        std::fs::write(&path, "hello world\nignore this\nhello rust").unwrap();
        let dir_str = dir.path().to_str().unwrap().to_string();

        let tool = GrepTool;
        let result = tool.call(json!({
            "pattern": "hello",
            "path": dir_str,
            "include": "*.txt"
        })).await;
        
        assert!(!result.is_error);
        assert!(result.content.contains("match.txt:1: hello world"));
        assert!(result.content.contains("match.txt:3: hello rust"));
    }

    #[tokio::test]
    async fn test_grep_no_match() {
        let dir = tempdir().unwrap();
        let dir_str = dir.path().to_str().unwrap().to_string();

        let tool = GrepTool;
        let result = tool.call(json!({
            "pattern": "impossible_string",
            "path": dir_str
        })).await;
        
        assert!(!result.is_error);
        assert!(result.content.contains("No matches found"));
    }

    #[tokio::test]
    async fn test_grep_invalid_regex() {
        let tool = GrepTool;
        let result = tool.call(json!({
            "pattern": "[invalid_regex",
            "path": "."
        })).await;
        
        assert!(result.is_error);
        assert!(result.content.contains("Invalid Regex pattern"));
    }
}
