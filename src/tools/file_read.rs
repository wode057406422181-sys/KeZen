use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;
use tokio::fs;

use super::{Tool, ToolResult};

pub struct FileReadTool;

#[async_trait]
impl Tool for FileReadTool {
    fn name(&self) -> &str {
        "FileRead"
    }

    fn description(&self) -> &str {
        "Read file content with optional line offsets"
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the file to read"
                },
                "offset": {
                    "type": "number",
                    "description": "Line number to start reading from (1-indexed)"
                },
                "limit": {
                    "type": "number",
                    "description": "Number of lines to read"
                }
            },
            "required": ["file_path"]
        })
    }

    async fn call(&self, input: serde_json::Value) -> ToolResult {
        let file_path_str = match input.get("file_path").and_then(|v| v.as_str()) {
            Some(path) => path,
            None => {
                return ToolResult {
                    content: "Error: missing or invalid 'file_path'".to_string(),
                    is_error: true,
                }
            }
        };

        let offset = input.get("offset").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
        let limit = input.get("limit").and_then(|v| v.as_u64()).map(|v| v as usize);

        let path = PathBuf::from(file_path_str);

        // Use tokio::fs for async metadata check instead of blocking path.exists()
        let content = match fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    return ToolResult {
                        content: format!("File does not exist: {}", file_path_str),
                        is_error: true,
                    };
                }
                // Try reading as bytes to check if binary
                if let Ok(bytes) = fs::read(&path).await {
                    if bytes.windows(2).any(|w| w == b"\0\0") {
                        return ToolResult {
                            content: format!("Cannot read binary file: {}", file_path_str),
                            is_error: true,
                        };
                    }
                }
                return ToolResult {
                    content: format!("Failed to read file: {}", e),
                    is_error: true,
                };
            }
        };

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        let start_idx = offset.saturating_sub(1);
        if start_idx >= total_lines && total_lines > 0 {
            return ToolResult {
                content: format!("<system-reminder>Warning: the file exists but is shorter than the provided offset ({}). The file has {} lines.</system-reminder>", offset, total_lines),
                is_error: false,
            };
        } else if total_lines == 0 {
            return ToolResult {
                content: "<system-reminder>Warning: the file exists but the contents are empty.</system-reminder>".to_string(),
                is_error: false,
            };
        }

        let end_idx = limit.map_or(total_lines, |l| (start_idx + l).min(total_lines));
        
        let mut result_content = String::new();
        for (i, line) in lines[start_idx..end_idx].iter().enumerate() {
            result_content.push_str(&format!("{}: {}\n", start_idx + i + 1, line));
        }

        ToolResult {
            content: result_content,
            is_error: false,
        }
    }
}
