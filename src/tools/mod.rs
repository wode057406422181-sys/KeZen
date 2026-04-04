use async_trait::async_trait;

pub mod registry;
pub mod bash;
pub mod file_read;
pub mod file_write;
pub mod file_edit;
pub mod grep;
pub mod glob;

use crate::permissions::PermissionResult;

/// The output returned by a tool after execution.
pub struct ToolResult {
    /// Human-readable output text (shown to both LLM and user).
    pub content: String,
    /// Whether the tool execution failed.
    pub is_error: bool,
}

/// Defines a tool that can be registered and invoked by the agentic loop.
///
/// Each tool provides its own JSON schema for the LLM to generate inputs,
/// and an async `call` method for execution. Tools must be `Send + Sync`
/// to work across tokio tasks.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Unique identifier used by the LLM to select this tool.
    fn name(&self) -> &str;
    /// Short description included in the tool schema sent to the LLM.
    fn description(&self) -> &str;
    /// JSON Schema describing the expected input parameters.
    fn input_schema(&self) -> serde_json::Value;
    /// Execute the tool with the given JSON input.
    async fn call(&self, input: serde_json::Value) -> ToolResult;

    /// Whether this tool's execution is strictly read-only for the given input.
    /// Read-only tools are auto-approved without prompting the user.
    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        false
    }

    /// Whether this tool operates on files (FileRead/FileWrite/FileEdit).
    /// Used by AcceptEdits mode to auto-approve safe file operations.
    fn is_file_tool(&self) -> bool {
        false
    }

    /// Tool-specific permission check based on the actual input parameters.
    ///
    /// This is where tools implement fine-grained safety checks:
    /// - Bash: read-only command detection, dangerous command warnings
    /// - FileWrite/FileEdit: path traversal, dangerous paths, working dir
    /// - FileRead/Grep/Glob: always Allow (read-only)
    ///
    /// Returns `Passthrough` by default (defer to generic pipeline).
    async fn check_permissions(&self, _input: &serde_json::Value) -> PermissionResult {
        PermissionResult::Passthrough
    }

    /// Returns a matcher function that checks if a permission rule's content
    /// matches this tool's input.
    ///
    /// Examples:
    /// - Bash: `"git commit:*"` matches commands starting with `git commit`
    /// - FileWrite: `"src/**"` matches files under `src/`
    ///
    /// Returns `None` by default (no content-level matching).
    #[allow(clippy::type_complexity)]
    fn permission_matcher(&self, _input: &serde_json::Value) -> Option<Box<dyn Fn(&str) -> bool + '_>> {
        None
    }

    /// Human-readable description for permission prompts.
    fn permission_description(&self, _input: &serde_json::Value) -> String {
        format!("{} wants to execute", self.name())
    }

    /// Extract a suggested always-allow rule content for the permission prompt.
    /// E.g. Bash returns `"git commit:*"` from `"git commit -m 'fix'"`.
    fn permission_suggestion(&self, _input: &serde_json::Value) -> Option<String> {
        None
    }
}
