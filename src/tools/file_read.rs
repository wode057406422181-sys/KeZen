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
                return ToolResult::err("Error: missing or invalid 'file_path'".to_string())
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
                    return ToolResult::err(format!("File does not exist: {}", file_path_str));
                }
                // Try reading as bytes to check if binary (check first 8KB for null bytes, like git)
                if let Ok(bytes) = fs::read(&path).await {
                    let check_len = bytes.len().min(8192);
                    if bytes[..check_len].contains(&0) {
                        return ToolResult::err(format!("Cannot read binary file: {}", file_path_str));
                    }
                }
                return ToolResult::err(format!("Failed to read file: {}", e));
            }
        };

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        let start_idx = offset.saturating_sub(1);
        if start_idx >= total_lines && total_lines > 0 {
            return ToolResult::ok(format!("<system-reminder>Warning: the file exists but is shorter than the provided offset ({}). The file has {} lines.</system-reminder>", offset, total_lines));
        } else if total_lines == 0 {
            return ToolResult::ok("<system-reminder>Warning: the file exists but the contents are empty.</system-reminder>".to_string());
        }

        let end_idx = limit.map_or(total_lines, |l| (start_idx + l).min(total_lines));
        
        let mut result_content = String::new();
        for (i, line) in lines[start_idx..end_idx].iter().enumerate() {
            result_content.push_str(&format!("{}: {}\n", start_idx + i + 1, line));
        }

        ToolResult::ok(result_content)
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    async fn check_permissions(&self, _input: &serde_json::Value) -> crate::permissions::PermissionResult {
        crate::permissions::PermissionResult::Allow
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_read_existing_file() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "line1\nline2\nline3").unwrap();
        let path_str = file.path().to_str().unwrap().to_string();

        let tool = FileReadTool;
        let result = tool.call(json!({"file_path": path_str})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("1: line1"));
        assert!(result.content.contains("3: line3"));
    }

    #[tokio::test]
    async fn test_read_nonexistent() {
        let tool = FileReadTool;
        let result = tool.call(json!({"file_path": "/path/to/nonexistent/file_12345.txt"})).await;
        assert!(result.is_error);
        assert!(result.content.contains("File does not exist:"));
    }

    #[tokio::test]
    async fn test_read_with_offset_and_limit() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "L1\nL2\nL3\nL4\nL5").unwrap();
        let path_str = file.path().to_str().unwrap().to_string();

        let tool = FileReadTool;
        let result = tool.call(json!({
            "file_path": path_str,
            "offset": 2,
            "limit": 2
        })).await;
        assert!(!result.is_error);
        assert!(!result.content.contains("1: L1"));
        assert!(result.content.contains("2: L2"));
        assert!(result.content.contains("3: L3"));
        assert!(!result.content.contains("4: L4"));
    }
}
