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

    fn check_permissions(&self, input: &serde_json::Value) -> crate::permissions::PermissionResult {
        let command = input.get("command").and_then(|v| v.as_str()).unwrap_or("");

        // Read-only commands are always safe
        if crate::permissions::safety::is_read_only_command(command) {
            return crate::permissions::PermissionResult::Allow;
        }

        // Dangerous paths in the command → ask
        if crate::permissions::safety::is_dangerous_path(command) {
            return crate::permissions::PermissionResult::Ask {
                message: format!("⚠️ Command may modify sensitive files: `{}`", command),
            };
        }

        // Default: defer to pipeline
        crate::permissions::PermissionResult::Passthrough
    }

    fn is_destructive(&self, input: &serde_json::Value) -> bool {
        let command = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
        let trimmed = command.trim();
        trimmed.starts_with("rm ")
            || trimmed.starts_with("rm\t")
            || trimmed.starts_with("rmdir ")
            || trimmed.contains("rm -rf")
            || trimmed.contains("rm -fr")
    }

    fn permission_matcher(&self, input: &serde_json::Value) -> Option<Box<dyn Fn(&str) -> bool + '_>> {
        let command = input.get("command").and_then(|v| v.as_str()).unwrap_or("").to_string();
        Some(Box::new(move |pattern: &str| {
            // Support "git commit:*" prefix matching
            if let Some(prefix) = pattern.strip_suffix(":*") {
                command.starts_with(prefix)
            } else {
                command == pattern
            }
        }))
    }

    fn permission_suggestion(&self, input: &serde_json::Value) -> Option<String> {
        let command = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
        crate::permissions::safety::extract_bash_suggestion(command)
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

    // ── check_permissions tests ──────────────────────────────────────

    #[test]
    fn test_check_permissions_read_only_allow() {
        let tool = BashTool;
        let input = json!({"command": "ls -la"});
        let result = tool.check_permissions(&input);
        assert!(matches!(result, crate::permissions::PermissionResult::Allow));
    }

    #[test]
    fn test_check_permissions_git_status_allow() {
        let tool = BashTool;
        let input = json!({"command": "git status"});
        let result = tool.check_permissions(&input);
        assert!(matches!(result, crate::permissions::PermissionResult::Allow));
    }

    #[test]
    fn test_check_permissions_cargo_test_allow() {
        let tool = BashTool;
        let input = json!({"command": "cargo test --release"});
        let result = tool.check_permissions(&input);
        assert!(matches!(result, crate::permissions::PermissionResult::Allow));
    }

    #[test]
    fn test_check_permissions_dangerous_path_ask() {
        let tool = BashTool;
        // Use a write command targeting a dangerous path
        // (cat .bashrc is read-only so it returns Allow before the danger check)
        let input = json!({"command": "sed -i 's/old/new/g' /home/user/.bashrc"});
        let result = tool.check_permissions(&input);
        assert!(matches!(result, crate::permissions::PermissionResult::Ask { .. }));
    }

    #[test]
    fn test_check_permissions_write_command_passthrough() {
        let tool = BashTool;
        // cargo build is not in the read-only list, and not a dangerous path
        let input = json!({"command": "cargo build --release"});
        let result = tool.check_permissions(&input);
        assert!(matches!(result, crate::permissions::PermissionResult::Passthrough));
    }

    #[test]
    fn test_check_permissions_git_push_passthrough() {
        let tool = BashTool;
        let input = json!({"command": "git push origin main"});
        let result = tool.check_permissions(&input);
        assert!(matches!(result, crate::permissions::PermissionResult::Passthrough));
    }

    // ── is_destructive tests ─────────────────────────────────────────

    #[test]
    fn test_is_destructive_rm() {
        let tool = BashTool;
        assert!(tool.is_destructive(&json!({"command": "rm -rf /tmp/foo"})));
        assert!(tool.is_destructive(&json!({"command": "rm file.txt"})));
        assert!(tool.is_destructive(&json!({"command": "rmdir empty_dir"})));
    }

    #[test]
    fn test_is_not_destructive_normal() {
        let tool = BashTool;
        assert!(!tool.is_destructive(&json!({"command": "ls -la"})));
        assert!(!tool.is_destructive(&json!({"command": "git commit -m 'fix'"})));
        assert!(!tool.is_destructive(&json!({"command": "cargo build"})));
    }

    // ── permission_matcher tests ─────────────────────────────────────

    #[test]
    fn test_matcher_prefix_match() {
        let tool = BashTool;
        let input = json!({"command": "git commit -m 'fix typo'"});
        let matcher = tool.permission_matcher(&input).unwrap();
        assert!(matcher("git commit:*"));
        assert!(!matcher("git push:*"));
    }

    #[test]
    fn test_matcher_exact_match() {
        let tool = BashTool;
        let input = json!({"command": "cargo build"});
        let matcher = tool.permission_matcher(&input).unwrap();
        assert!(matcher("cargo build"));
        assert!(!matcher("cargo test"));
    }

    #[test]
    fn test_matcher_broad_prefix() {
        let tool = BashTool;
        let input = json!({"command": "npm run dev"});
        let matcher = tool.permission_matcher(&input).unwrap();
        assert!(matcher("npm:*")); // matches any npm command
        assert!(matcher("npm run:*")); // matches npm run subcommand
        assert!(!matcher("yarn:*"));
    }

    // ── permission_suggestion tests ──────────────────────────────────

    #[test]
    fn test_suggestion_multi_word() {
        let tool = BashTool;
        let input = json!({"command": "git commit -m 'fix'"});
        assert_eq!(tool.permission_suggestion(&input), Some("git commit:*".into()));
    }

    #[test]
    fn test_suggestion_single_word() {
        let tool = BashTool;
        let input = json!({"command": "make"});
        assert_eq!(tool.permission_suggestion(&input), Some("make:*".into()));
    }
}
