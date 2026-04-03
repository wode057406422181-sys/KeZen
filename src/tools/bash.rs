use async_trait::async_trait;
use serde_json::json;
use tokio::process::Command;

use super::{Tool, ToolResult};

pub struct BashTool;

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "Bash"
    }

    fn description(&self) -> &str {
        "Run shell command"
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute"
                },
                "timeout": {
                    "type": "number",
                    "description": "Optional timeout in milliseconds"
                }
            },
            "required": ["command"]
        })
    }

    async fn call(&self, input: serde_json::Value) -> ToolResult {
        let command = match input.get("command").and_then(|v| v.as_str()) {
            Some(cmd) => cmd,
            None => {
                return ToolResult {
                    content: "Error: missing or invalid 'command' parameter".to_string(),
                    is_error: true,
                }
            }
        };

        let timeout_ms = input.get("timeout").and_then(|v| v.as_u64()).unwrap_or(30_000);
        let timeout_duration = std::time::Duration::from_millis(timeout_ms);

        let child = Command::new("bash")
            .arg("-c")
            .arg(command)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn();

        let child_proc = match child {
            Ok(c) => c,
            Err(e) => {
                return ToolResult {
                    content: format!("Failed to spawn shell: {}", e),
                    is_error: true,
                }
            }
        };

        let result = tokio::time::timeout(timeout_duration, child_proc.wait_with_output()).await;

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);

                let mut content = format!("{}{}", stdout, stderr);
                let is_error = !output.status.success();

                if is_error {
                    if !content.ends_with('\n') && !content.is_empty() {
                        content.push('\n');
                    }
                    content.push_str(&format!("Exit code {}", output.status.code().unwrap_or(1)));
                }

                ToolResult {
                    content,
                    is_error,
                }
            }
            Ok(Err(e)) => ToolResult {
                content: format!("Failed to execute command: {}", e),
                is_error: true,
            },
            Err(_) => {
                ToolResult {
                    content: format!("Command killed due to timeout of {}ms", timeout_ms),
                    is_error: true,
                }
            }
        }
    }

    fn permission_description(&self, input: &serde_json::Value) -> String {
        let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("unknown");
        format!("Run: `{}`", cmd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_bash_echo() {
        let tool = BashTool;
        let result = tool.call(json!({"command": "echo 'hello world'"})).await;
        assert!(!result.is_error);
        assert_eq!(result.content.trim(), "hello world");
    }

    #[tokio::test]
    async fn test_bash_exit_code() {
        let tool = BashTool;
        let result = tool.call(json!({"command": "exit 1"})).await;
        assert!(result.is_error);
        assert!(result.content.contains("Exit code 1"));
    }

    #[tokio::test]
    async fn test_bash_timeout() {
        let tool = BashTool;
        let result = tool.call(json!({"command": "sleep 1", "timeout": 10})).await;
        assert!(result.is_error);
        assert!(result.content.contains("timeout of 10ms"));
    }

    #[tokio::test]
    async fn test_bash_missing_command() {
        let tool = BashTool;
        let result = tool.call(json!({})).await;
        assert!(result.is_error);
        assert!(result.content.contains("missing or invalid 'command'"));
    }
}
