pub mod anthropic;
pub mod debug_logger;
pub mod openai;
pub mod types;

use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;

use crate::api::types::{Message, StreamEvent};
use crate::config::{AppConfig, Provider};
use crate::error::InfiniError;

/// A pinned, boxed, Send stream of StreamEvent results.
pub type BoxStream<'a, T> = Pin<Box<dyn Stream<Item = Result<T, InfiniError>> + Send + 'a>>;

/// Unified LLM client interface.
///
/// Both Anthropic and OpenAI providers implement this trait.
/// The Engine only interacts with this abstraction.
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Start a streaming completion request.
    ///
    /// Returns a stream of `StreamEvent` items that the Engine consumes.
    async fn stream(
        &self,
        messages: &[Message],
        system_prompt: Option<&str>,
        tools: Option<&[serde_json::Value]>,
    ) -> Result<BoxStream<'_, StreamEvent>, InfiniError>;
}

/// Factory function: create the appropriate LLM client based on config.
pub fn create_client(config: &AppConfig) -> Result<Box<dyn LlmClient>, InfiniError> {
    match config.provider {
        Provider::Anthropic => Ok(Box::new(anthropic::AnthropicClient::new(config)?)),
        Provider::OpenAi => Ok(Box::new(openai::OpenAiClient::new(config)?)),
    }
}
