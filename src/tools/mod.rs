use async_trait::async_trait;

pub mod registry;
pub mod bash;
pub mod file_read;
pub mod file_write;
pub mod file_edit;
pub mod grep;
pub mod glob;

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
}
