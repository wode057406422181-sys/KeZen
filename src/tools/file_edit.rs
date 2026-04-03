use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;
use tokio::fs;

use super::{Tool, ToolResult};

pub struct FileEditTool;

#[async_trait]
impl Tool for FileEditTool {
    fn name(&self) -> &str {
        "FileEdit"
    }

    fn description(&self) -> &str {
        "Modify file contents in place"
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the file to edit"
                },
                "old_string": {
                    "type": "string",
                    "description": "The string to replace"
                },
                "new_string": {
                    "type": "string",
                    "description": "The replacement string"
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "If true, replace all occurrences"
                }
            },
            "required": ["file_path", "old_string", "new_string"]
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

        let old_string = match input.get("old_string").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => {
                return ToolResult {
                    content: "Error: missing or invalid 'old_string'".to_string(),
                    is_error: true,
                }
            }
        };

        let new_string = match input.get("new_string").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => {
                return ToolResult {
                    content: "Error: missing or invalid 'new_string'".to_string(),
                    is_error: true,
                }
            }
        };

        let replace_all = input.get("replace_all").and_then(|v| v.as_bool()).unwrap_or(false);

        let path = PathBuf::from(file_path);
        let file_exists = fs::metadata(&path).await.is_ok();
        
        let content = match fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(_) => {
                if !file_exists && old_string.is_empty() {
                    String::new()
                } else {
                    return ToolResult {
                        content: format!("Failed to read file: {}", file_path),
                        is_error: true,
                    };
                }
            }
        };

        if old_string.is_empty() && !file_exists {
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent).await;
            }
            if let Err(e) = fs::write(&path, new_string).await {
                return ToolResult {
                    content: format!("Failed to write new file: {}", e),
                    is_error: true,
                };
            }
            return ToolResult {
                content: format!("The file {} has been created successfully.", file_path),
                is_error: false,
            };
        }

        if !content.contains(old_string) {
            return ToolResult {
                content: format!("String to replace not found in file.\nString: {}", old_string),
                is_error: true,
            };
        }

        let occurrences = content.matches(old_string).count();
        if occurrences > 1 && !replace_all {
            return ToolResult {
                content: format!("Found {} matches of the string to replace, but replace_all is false. To replace all occurrences, set replace_all to true. To replace only one occurrence, please provide more context to uniquely identify the instance.\nString: {}", occurrences, old_string),
                is_error: true,
            };
        }

        let updated_content = if replace_all {
            content.replace(old_string, new_string)
        } else {
            content.replacen(old_string, new_string, 1)
        };

        if let Err(e) = fs::write(&path, updated_content).await {
            return ToolResult {
                content: format!("Failed to write updated file: {}", e),
                is_error: true,
            };
        }

        ToolResult {
            content: if replace_all {
                format!("The file {} has been updated. All occurrences were successfully replaced.", file_path)
            } else {
                format!("The file {} has been updated successfully.", file_path)
            },
            is_error: false,
        }
    }
}
