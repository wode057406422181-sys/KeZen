use async_trait::async_trait;

pub mod registry;
pub mod bash;
pub mod file_read;
pub mod file_write;
pub mod file_edit;
pub mod grep;
pub mod glob;

pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn input_schema(&self) -> serde_json::Value;

    async fn call(&self, input: serde_json::Value) -> ToolResult;
}
