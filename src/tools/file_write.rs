use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;
use tokio::fs;

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
            if let Err(e) = fs::create_dir_all(parent).await {
                return ToolResult {
                    content: format!("Failed to create parent directories: {}", e),
                    is_error: true,
                };
            }
        }

        // Use async metadata check instead of blocking path.exists()
        let is_create = fs::metadata(&path).await.is_err();

        match fs::write(&path, content).await {
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_write_creates_new_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test_new.txt");
        let path_str = path.to_str().unwrap().to_string();

        let tool = FileWriteTool;
        let result = tool.call(json!({
            "file_path": path_str,
            "content": "hello core"
        })).await;
        
        assert!(!result.is_error);
        assert!(result.content.contains("created successfully"));
        
        let read_content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(read_content, "hello core");
    }

    #[tokio::test]
    async fn test_write_creates_parent_dirs() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nested").join("dir").join("file.txt");
        let path_str = path.to_str().unwrap().to_string();

        let tool = FileWriteTool;
        let result = tool.call(json!({
            "file_path": path_str,
            "content": "nested content"
        })).await;
        
        assert!(!result.is_error);
        assert!(path.exists());
    }

    #[tokio::test]
    async fn test_write_overwrites_existing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("exist.txt");
        std::fs::write(&path, "old").unwrap();
        let path_str = path.to_str().unwrap().to_string();

        let tool = FileWriteTool;
        let result = tool.call(json!({
            "file_path": path_str,
            "content": "new"
        })).await;
        
        assert!(!result.is_error);
        assert!(result.content.contains("updated successfully"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
    }
}
