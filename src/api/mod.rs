pub mod anthropic;
pub mod debug_logger;
pub mod openai;
pub mod types;

use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;

use crate::api::types::{Message, StreamEvent};
use crate::config::{AppConfig, Provider};
use crate::error::KezenError;

/// A pinned, boxed, Send stream of StreamEvent results.
pub type BoxStream<'a, T> = Pin<Box<dyn Stream<Item = Result<T, KezenError>> + Send + 'a>>;

/// Provider-agnostic extra parameters for the LLM request.
///
/// These options are consumed by provider implementations to inject
/// provider-specific features (e.g. native web search).
#[derive(Default, Clone, Debug)]
pub struct StreamOptions {
    /// Enable server-side web search (DashScope `enable_search`, etc.).
    pub enable_server_search: bool,
    /// Enable server-side web fetch (Anthropic `web_fetch_20250910`, etc.).
    #[allow(dead_code)] // TODO: Implement server-side fetch in both Anthropic and OpenAI providers
    pub enable_server_fetch: bool,
    /// Search strategy hint for providers that support it.
    /// DashScope values: "turbo", "max", "agent", "agent_max".
    pub search_strategy: Option<String>,
}

/// Unified LLM client interface.
///
/// Both Anthropic and OpenAI providers implement this trait.
/// The Engine only interacts with this abstraction.
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Start a streaming completion request.
    ///
    /// Returns a stream of `StreamEvent` items that the Engine consumes.
    ///
    /// * `max_tokens_override` — When `Some(n)`, overrides the client's default
    ///   `max_tokens` for this single request (e.g. compact uses 20,000 as
    ///   `COMPACT_MAX_OUTPUT_TOKENS`).
    async fn stream(
        &self,
        messages: &[Message],
        system_prompt: Option<&str>,
        tools: Option<&[serde_json::Value]>,
        options: &StreamOptions,
        max_tokens_override: Option<u32>,
    ) -> Result<BoxStream<'_, StreamEvent>, KezenError>;
}

/// Factory function: create the appropriate LLM client based on config.
pub fn create_client(config: &AppConfig) -> Result<Box<dyn LlmClient>, KezenError> {
    match config.provider {
        Provider::Anthropic => Ok(Box::new(anthropic::AnthropicClient::new(config)?)),
        Provider::OpenAi => Ok(Box::new(openai::OpenAiClient::new(config)?)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_options_default_has_no_search() {
        let opts = StreamOptions::default();
        assert!(!opts.enable_server_search);
        assert!(!opts.enable_server_fetch);
        assert!(opts.search_strategy.is_none());
    }

    #[test]
    fn stream_options_clone_preserves_fields() {
        let opts = StreamOptions {
            enable_server_search: true,
            enable_server_fetch: true,
            search_strategy: Some("agent_max".to_string()),
        };
        let cloned = opts.clone();
        assert!(cloned.enable_server_search);
        assert!(cloned.enable_server_fetch);
        assert_eq!(cloned.search_strategy.as_deref(), Some("agent_max"));
    }

    #[test]
    fn stream_options_debug_output() {
        let opts = StreamOptions {
            enable_server_search: true,
            enable_server_fetch: false,
            search_strategy: Some("turbo".to_string()),
        };
        let debug = format!("{:?}", opts);
        assert!(debug.contains("enable_server_search: true"));
        assert!(debug.contains("enable_server_fetch: false"));
        assert!(debug.contains("turbo"));
    }
}
