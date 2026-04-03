use async_trait::async_trait;
use serde_json::json;
use std::fs;
use std::path::PathBuf;

use super::{Tool, ToolResult};

pub struct FileWriteTool;

#[async_trait]
impl Tool for FileWriteTool {
    fn name(&self) -> &str {
        "FileWrite"
    }

    fn description(&self) -> &str {
        "Write a file to the local filesystem"
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The absolute path to the file to write"
                },
                "content": {
                    "type": "string",
                    "description": "The content to write to the file"
                }
            },
            "required": ["file_path", "content"]
        })
    }

    async fn call(&self, input: serde_json::Value) -> ToolResult {
        let file_path = match input.get("file_path").and_then(|v| v.as_str()) {
            Some(path) => path,
            None => {
                return ToolResult {
                    content: "Error: missing or invalid 'file_path'".to_string(),
                    is_error: true,
                }
            }
        };

        let content = match input.get("content").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => {
                return ToolResult {
                    content: "Error: missing or invalid 'content'".to_string(),
                    is_error: true,
                }
            }
        };

        let path = PathBuf::from(file_path);
        if let Some(parent) = path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                return ToolResult {
                    content: format!("Failed to create parent directories: {}", e),
                    is_error: true,
                };
            }
        }

        let is_create = !path.exists();

        match fs::write(&path, content) {
            Ok(_) => ToolResult {
                content: if is_create {
                    format!("File created successfully at: {}", file_path)
                } else {
                    format!("The file {} has been updated successfully.", file_path)
                },
                is_error: false,
            },
            Err(e) => ToolResult {
                content: format!("Failed to write file: {}", e),
                is_error: true,
            },
        }
    }
}
