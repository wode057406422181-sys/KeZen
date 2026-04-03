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
        if let Some(parent) = path.parent()
            && let Err(e) = fs::create_dir_all(parent).await {
                return ToolResult {
                    content: format!("Failed to create parent directories: {}", e),
                    is_error: true,
                };
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

    fn permission_description(&self, input: &serde_json::Value) -> String {
        let path = input.get("file_path").and_then(|v| v.as_str()).unwrap_or("unknown");
        format!("Create/overwrite file: {}", path)
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        false
    }

    fn is_file_tool(&self) -> bool {
        true
    }

    fn check_permissions(&self, input: &serde_json::Value) -> crate::permissions::PermissionResult {
        let file_path = input.get("file_path").and_then(|v| v.as_str()).unwrap_or("");

        // Path traversal → deny
        if crate::permissions::safety::contains_path_traversal(file_path) {
            return crate::permissions::PermissionResult::Deny {
                message: format!("Path contains traversal (..): {}", file_path),
            };
        }

        // Dangerous files (.git/, .bashrc, etc.) → ask
        if crate::permissions::safety::is_dangerous_path(file_path) {
            return crate::permissions::PermissionResult::Ask {
                message: format!("⚠️ Target is a sensitive file: {}", file_path),
            };
        }

        // Working directory check
        if let Ok(cwd) = std::env::current_dir() {
            let cwd_str = cwd.to_string_lossy();
            if !crate::permissions::safety::is_within_working_directory(file_path, &cwd_str) {
                return crate::permissions::PermissionResult::Ask {
                    message: format!("⚠️ File is outside the working directory: {}", file_path),
                };
            }
        }

        crate::permissions::PermissionResult::Passthrough
    }

    fn permission_matcher(&self, input: &serde_json::Value) -> Option<Box<dyn Fn(&str) -> bool + '_>> {
        let path = input.get("file_path").and_then(|v| v.as_str()).unwrap_or("").to_string();
        Some(Box::new(move |pattern: &str| {
            // Support "src/**" glob matching
            if let Some(prefix) = pattern.strip_suffix("/**") {
                // Check if the file is under the directory prefix
                let path_obj = std::path::Path::new(&path);
                let mut components = path_obj.components();
                components.any(|c| {
                    if let std::path::Component::Normal(s) = c {
                        s.to_str() == Some(prefix)
                    } else {
                        false
                    }
                })
            } else {
                path == pattern
            }
        }))
    }

    fn permission_suggestion(&self, input: &serde_json::Value) -> Option<String> {
        let file_path = input.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
        if let Ok(cwd) = std::env::current_dir() {
            crate::permissions::safety::extract_file_suggestion(file_path, &cwd.to_string_lossy())
        } else {
            None
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

    // ── check_permissions tests ──────────────────────────────────────

    #[test]
    fn test_check_permissions_path_traversal_deny() {
        let tool = FileWriteTool;
        let input = json!({"file_path": "/project/../etc/passwd", "content": "x"});
        let result = tool.check_permissions(&input);
        assert!(matches!(result, crate::permissions::PermissionResult::Deny { .. }));
    }

    #[test]
    fn test_check_permissions_dangerous_path_ask() {
        let tool = FileWriteTool;
        let input = json!({"file_path": "/project/.git/hooks/post-commit", "content": "x"});
        let result = tool.check_permissions(&input);
        assert!(matches!(result, crate::permissions::PermissionResult::Ask { .. }));
    }

    #[test]
    fn test_check_permissions_bashrc_ask() {
        let tool = FileWriteTool;
        let input = json!({"file_path": "/home/user/.bashrc", "content": "x"});
        let result = tool.check_permissions(&input);
        assert!(matches!(result, crate::permissions::PermissionResult::Ask { .. }));
    }

    #[test]
    fn test_check_permissions_normal_path() {
        let tool = FileWriteTool;
        // Use current working directory so path is within it
        let cwd = std::env::current_dir().unwrap();
        let file = cwd.join("src").join("test_output.rs");
        let input = json!({"file_path": file.to_str().unwrap(), "content": "x"});
        let result = tool.check_permissions(&input);
        // Should be Passthrough (safe file in working dir)
        assert!(matches!(result, crate::permissions::PermissionResult::Passthrough));
    }

    // ── trait method tests ───────────────────────────────────────────

    #[test]
    fn test_is_file_tool() {
        let tool = FileWriteTool;
        assert!(tool.is_file_tool());
    }

    #[test]
    fn test_is_not_read_only() {
        let tool = FileWriteTool;
        assert!(!tool.is_read_only(&json!({})));
    }

    // ── permission_matcher tests ─────────────────────────────────────

    #[test]
    fn test_matcher_glob() {
        let tool = FileWriteTool;
        let input = json!({"file_path": "/project/src/main.rs"});
        let matcher = tool.permission_matcher(&input).unwrap();
        assert!(matcher("src/**")); // matches src directory
        assert!(!matcher("tests/**")); // does not match tests directory
    }

    #[test]
    fn test_matcher_exact_path() {
        let tool = FileWriteTool;
        let input = json!({"file_path": "/project/README.md"});
        let matcher = tool.permission_matcher(&input).unwrap();
        assert!(matcher("/project/README.md")); // exact match
        assert!(!matcher("/project/LICENSE")); // different file
    }
}
