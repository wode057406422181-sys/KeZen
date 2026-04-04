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
                return ToolResult::err("Error: missing or invalid 'file_path'".to_string())
            }
        };

        let content = match input.get("content").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => {
                return ToolResult::err("Error: missing or invalid 'content'".to_string())
            }
        };

        let path = PathBuf::from(file_path);
        if let Some(parent) = path.parent()
            && let Err(e) = fs::create_dir_all(parent).await {
                return ToolResult::err(format!("Failed to create parent directories: {}", e));
            }

        // Use async metadata check instead of blocking path.exists()
        let is_create = fs::metadata(&path).await.is_err();

        match fs::write(&path, content).await {
            Ok(_) => ToolResult::ok(if is_create {
                format!("File created successfully at: {}", file_path)
            } else {
                format!("The file {} has been updated successfully.", file_path)
            }),
            Err(e) => ToolResult::err(format!("Failed to write file: {}", e)),
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

    async fn check_permissions(&self, input: &serde_json::Value) -> crate::permissions::PermissionResult {
        let file_path = input.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
        crate::permissions::safety::check_file_permissions(file_path).await
    }

    fn permission_matcher(&self, input: &serde_json::Value) -> Option<Box<dyn Fn(&str) -> bool + '_>> {
        let path = input.get("file_path").and_then(|v| v.as_str()).unwrap_or("").to_string();
        Some(crate::permissions::safety::file_permission_matcher(path))
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

    #[tokio::test]
    async fn test_check_permissions_path_traversal_deny() {
        let tool = FileWriteTool;
        let input = json!({"file_path": "/project/../etc/passwd", "content": "x"});
        let result = tool.check_permissions(&input).await;
        assert!(matches!(result, crate::permissions::PermissionResult::Deny { .. }));
    }

    #[tokio::test]
    async fn test_check_permissions_dangerous_path_ask() {
        let tool = FileWriteTool;
        let input = json!({"file_path": "/project/.git/hooks/post-commit", "content": "x"});
        let result = tool.check_permissions(&input).await;
        assert!(matches!(result, crate::permissions::PermissionResult::Ask { .. }));
    }

    #[tokio::test]
    async fn test_check_permissions_bashrc_ask() {
        let tool = FileWriteTool;
        let input = json!({"file_path": "/home/user/.bashrc", "content": "x"});
        let result = tool.check_permissions(&input).await;
        assert!(matches!(result, crate::permissions::PermissionResult::Ask { .. }));
    }

    #[tokio::test]
    async fn test_check_permissions_normal_path() {
        let tool = FileWriteTool;
        let cwd = std::env::current_dir().unwrap();
        let file = cwd.join("src").join("test_output.rs");
        let input = json!({"file_path": file.to_str().unwrap(), "content": "x"});
        let result = tool.check_permissions(&input).await;
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
        let cwd = std::env::current_dir().unwrap();
        let file_path = format!("{}/src/main.rs", cwd.display());
        let input = json!({"file_path": file_path});
        let matcher = tool.permission_matcher(&input).unwrap();
        assert!(matcher("src/**")); // relative: resolves to <cwd>/src/
        assert!(!matcher("tests/**")); // does not match tests directory
    }

    #[test]
    fn test_matcher_absolute_glob() {
        let tool = FileWriteTool;
        let input = json!({"file_path": "/tmp/project/src/main.rs"});
        let matcher = tool.permission_matcher(&input).unwrap();
        assert!(matcher("/tmp/project/src/**")); // absolute prefix
        assert!(!matcher("/other/**")); // different root
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
