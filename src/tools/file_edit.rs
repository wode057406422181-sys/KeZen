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
                return ToolResult::err("Error: missing or invalid 'file_path'".to_string())
            }
        };

        let old_string = match input.get("old_string").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => {
                return ToolResult::err("Error: missing or invalid 'old_string'".to_string())
            }
        };

        let new_string = match input.get("new_string").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => {
                return ToolResult::err("Error: missing or invalid 'new_string'".to_string())
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
                    return ToolResult::err(format!("Failed to read file: {}", file_path));
                }
            }
        };

        if old_string.is_empty() {
            if file_exists {
                return ToolResult::err("Error: old_string cannot be empty for an existing file. Use FileWrite to overwrite the entire file, or provide a non-empty old_string to target a specific section.".to_string());
            }
            // Create new file
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent).await;
            }
            if let Err(e) = fs::write(&path, new_string).await {
                return ToolResult::err(format!("Failed to write new file: {}", e));
            }
            return ToolResult::ok(format!("The file {} has been created successfully.", file_path));
        }

        if !content.contains(old_string) {
            return ToolResult::err(format!("String to replace not found in file.\nString: {}", old_string));
        }

        let occurrences = content.matches(old_string).count();
        if occurrences > 1 && !replace_all {
            return ToolResult::err(format!("Found {} matches of the string to replace, but replace_all is false. To replace all occurrences, set replace_all to true. To replace only one occurrence, please provide more context to uniquely identify the instance.\nString: {}", occurrences, old_string));
        }

        let updated_content = if replace_all {
            content.replace(old_string, new_string)
        } else {
            content.replacen(old_string, new_string, 1)
        };

        if let Err(e) = fs::write(&path, updated_content).await {
            return ToolResult::err(format!("Failed to write updated file: {}", e));
        }

        ToolResult::ok(if replace_all {
            format!("The file {} has been updated. All occurrences were successfully replaced.", file_path)
        } else {
            format!("The file {} has been updated successfully.", file_path)
        })
    }

    fn permission_description(&self, input: &serde_json::Value) -> String {
        let path = input.get("file_path").and_then(|v| v.as_str()).unwrap_or("unknown");
        format!("Edit file: {}", path)
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
    async fn test_edit_replaces_single() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("edit.txt");
        std::fs::write(&path, "hello world").unwrap();
        let path_str = path.to_str().unwrap().to_string();

        let tool = FileEditTool;
        let result = tool.call(json!({
            "file_path": path_str,
            "old_string": "world",
            "new_string": "rust",
            "replace_all": false
        })).await;

        assert!(!result.is_error);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello rust");
    }

    #[tokio::test]
    async fn test_edit_multiple_without_flag_errors() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("edit_mult.txt");
        std::fs::write(&path, "apple apple banana").unwrap();
        let path_str = path.to_str().unwrap().to_string();

        let tool = FileEditTool;
        let result = tool.call(json!({
            "file_path": path_str,
            "old_string": "apple",
            "new_string": "orange",
            "replace_all": false
        })).await;

        assert!(result.is_error);
        assert!(result.content.contains("Found 2 matches"));
    }

    #[tokio::test]
    async fn test_edit_replace_all() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("edit_all.txt");
        std::fs::write(&path, "foo bar foo").unwrap();
        let path_str = path.to_str().unwrap().to_string();

        let tool = FileEditTool;
        let result = tool.call(json!({
            "file_path": path_str,
            "old_string": "foo",
            "new_string": "baz",
            "replace_all": true
        })).await;

        assert!(!result.is_error);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "baz bar baz");
    }

    #[tokio::test]
    async fn test_edit_not_found() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("not_found.txt");
        std::fs::write(&path, "hello").unwrap();
        let path_str = path.to_str().unwrap().to_string();

        let tool = FileEditTool;
        let result = tool.call(json!({
            "file_path": path_str,
            "old_string": "world",
            "new_string": "rust"
        })).await;

        assert!(result.is_error);
        assert!(result.content.contains("not found in file"));
    }

    // ── check_permissions tests ──────────────────────────────────────

    #[tokio::test]
    async fn test_check_permissions_path_traversal_deny() {
        let tool = FileEditTool;
        let input = json!({"file_path": "/project/../etc/shadow", "old_string": "x", "new_string": "y"});
        let result = tool.check_permissions(&input).await;
        assert!(matches!(result, crate::permissions::PermissionResult::Deny { .. }));
    }

    #[tokio::test]
    async fn test_check_permissions_dangerous_path_ask() {
        let tool = FileEditTool;
        let input = json!({"file_path": "/project/.git/config", "old_string": "x", "new_string": "y"});
        let result = tool.check_permissions(&input).await;
        assert!(matches!(result, crate::permissions::PermissionResult::Ask { .. }));
    }

    #[tokio::test]
    async fn test_check_permissions_gitmodules_ask() {
        let tool = FileEditTool;
        let input = json!({"file_path": "/project/.gitmodules", "old_string": "x", "new_string": "y"});
        let result = tool.check_permissions(&input).await;
        assert!(matches!(result, crate::permissions::PermissionResult::Ask { .. }));
    }

    #[tokio::test]
    async fn test_check_permissions_normal_path_passthrough() {
        let tool = FileEditTool;
        let cwd = std::env::current_dir().unwrap();
        let file = cwd.join("src").join("main.rs");
        let input = json!({"file_path": file.to_str().unwrap(), "old_string": "x", "new_string": "y"});
        let result = tool.check_permissions(&input).await;
        assert!(matches!(result, crate::permissions::PermissionResult::Passthrough));
    }

    // ── trait method tests ───────────────────────────────────────────

    #[test]
    fn test_is_file_tool() {
        let tool = FileEditTool;
        assert!(tool.is_file_tool());
    }

    #[test]
    fn test_is_not_read_only() {
        let tool = FileEditTool;
        assert!(!tool.is_read_only(&json!({})));
    }

    // ── permission_matcher tests ─────────────────────────────────────

    #[test]
    fn test_matcher_glob() {
        let tool = FileEditTool;
        let cwd = std::env::current_dir().unwrap();
        let file_path = format!("{}/src/lib.rs", cwd.display());
        let input = json!({"file_path": file_path});
        let matcher = tool.permission_matcher(&input).unwrap();
        assert!(matcher("src/**")); // relative: resolves to <cwd>/src/
        assert!(!matcher("tests/**"));
    }

    #[test]
    fn test_matcher_absolute_glob() {
        let tool = FileEditTool;
        let input = json!({"file_path": "/tmp/project/src/lib.rs"});
        let matcher = tool.permission_matcher(&input).unwrap();
        assert!(matcher("/tmp/project/src/**")); // absolute prefix
        assert!(!matcher("/other/**")); // different root
    }

    #[test]
    fn test_matcher_exact_path() {
        let tool = FileEditTool;
        let input = json!({"file_path": "/project/Cargo.toml"});
        let matcher = tool.permission_matcher(&input).unwrap();
        assert!(matcher("/project/Cargo.toml"));
        assert!(!matcher("/project/Cargo.lock"));
    }
}
