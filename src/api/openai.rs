use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::json;

use crate::api::debug_logger;
use crate::api::types::{ContentBlock, Message, Role, StreamEvent, Usage};
use crate::api::{BoxStream, LlmClient};
use crate::config::AppConfig;
use crate::constants::api::CONTENT_TYPE_JSON;
use crate::constants::defaults::DEFAULT_MAX_TOKENS;
use crate::error::InfiniError;

pub struct OpenAiClient {
    client: reqwest::Client,
    model: String,
    max_tokens: u32,
    /// Base URL, trimmed of trailing slashes.
    /// Must be a root URL (e.g. `https://api.openai.com`); path segments
    /// like `/v1/chat/completions` are appended automatically.
    base_url: String,
    /// Mirror of `AppConfig::include_stream_usage`.
    include_stream_usage: bool,
}

impl OpenAiClient {
    pub fn new(config: &AppConfig) -> Result<Self, InfiniError> {
        let api_key = config.api_key.as_deref().ok_or(InfiniError::NoApiKey)?;
        let model = config
            .model
            .as_deref()
            .ok_or(InfiniError::NoModel)?
            .to_string();

        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {}", api_key))
                .map_err(|e| InfiniError::Config(format!("Invalid API key format: {}", e)))?,
        );
        headers.insert(
            "x-api-key",
            HeaderValue::from_str(api_key)
                .map_err(|e| InfiniError::Config(format!("Invalid API key format: {}", e)))?,
        );
        headers.insert("content-type", HeaderValue::from_static(CONTENT_TYPE_JSON));
        headers.insert(
            "user-agent",
            HeaderValue::from_str(config.user_agent())
                .map_err(|e| InfiniError::Config(format!("Invalid User-Agent format: {}", e)))?,
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()?;

        let base_url = config.base_url().trim_end_matches('/').to_string();

        Ok(Self {
            client,
            model,
            max_tokens: config.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            base_url,
            include_stream_usage: config.include_stream_usage,
        })
    }
}

/// Normalise an OpenAI-compatible base URL to a full chat-completions endpoint.
///
/// Accepts any of:
/// - bare root:              `https://api.openai.com`
/// - with `/v1`:             `https://api.openai.com/v1`
/// - already full endpoint:  `https://api.openai.com/v1/chat/completions`
pub(crate) fn normalize_openai_url(base: &str) -> String {
    if base.ends_with("/v1/chat/completions") {
        base.to_string()
    } else if base.ends_with("/v1") {
        format!("{}/chat/completions", base)
    } else {
        format!("{}/v1/chat/completions", base)
    }
}

#[async_trait]
impl LlmClient for OpenAiClient {
    async fn stream(
        &self,
        messages: &[Message],
        system_prompt: Option<&str>,
    ) -> Result<BoxStream<'_, StreamEvent>, InfiniError> {
        let url = normalize_openai_url(&self.base_url);

        // Convert internal message format to OpenAI format
        let mut oai_messages = Vec::new();

        if let Some(sys_prompt) = system_prompt {
            oai_messages.push(json!({
                "role": "system",
                "content": sys_prompt
            }));
        }

        for msg in messages {
            let role_str = match msg.role {
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::System => "system",
            };

            // Join text blocks into single content string
            let mut text_content = String::new();
            for block in &msg.content {
                if let ContentBlock::Text { text } = block {
                    text_content.push_str(text);
                }
            }

            oai_messages.push(json!({
                "role": role_str,
                "content": text_content
            }));
        }

        let mut body = json!({
            "model": self.model,
            "max_completion_tokens": self.max_tokens,
            "messages": oai_messages,
            "stream": true,
        });

        // `stream_options.include_usage` is an OpenAI extension not supported by
        // all compatible endpoints (DashScope, Ollama, vLLM, etc.). Only send it
        // when explicitly enabled (default: true for the official OpenAI API).
        if self.include_stream_usage {
            body["stream_options"] = json!({"include_usage": true});
        }

        debug_logger::log_request("openai", &url, &body);

        let response = self.client.post(&url).json(&body).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            debug_logger::log_error_response("openai", status.as_u16(), &text);
            return Err(InfiniError::Api(format!(
                "OpenAI API error {}: {}",
                status, text
            )));
        }

        let stream = response.bytes_stream().eventsource();

        let event_stream = stream.filter_map(|event_result| async {
            match event_result {
                Ok(event) => {
                    debug_logger::log_sse_event("openai", "message", &event.data);
                    // OpenAI signals end of stream with [DONE]
                    if event.data == "[DONE]" {
                        return Some(Ok(StreamEvent::MessageStop));
                    }

                    let v: serde_json::Value = match serde_json::from_str(&event.data) {
                        Ok(v) => v,
                        Err(e) => return Some(Err(InfiniError::Json(e))),
                    };

                    // Extract usage from the final chunk (when choices is empty)
                    if v["usage"].is_object() && !v["usage"].is_null() {
                        let usage = Usage {
                            input_tokens: v["usage"]["prompt_tokens"].as_u64().unwrap_or(0),
                            output_tokens: v["usage"]["completion_tokens"].as_u64().unwrap_or(0),
                        };
                        // If this chunk also has no delta content, just return usage via MessageDelta
                        let has_content = v["choices"].as_array().is_some_and(|c| {
                            !c.is_empty() && c[0]["delta"]["content"].as_str().is_some()
                        });
                        if !has_content {
                            return Some(Ok(StreamEvent::MessageDelta {
                                stop_reason: None,
                                usage: Some(usage),
                            }));
                        }
                    }

                    // Extract text delta from choices
                    if let Some(choices) = v["choices"].as_array()
                        && !choices.is_empty()
                    {
                        // Check for finish_reason
                        if let Some(reason) = choices[0]["finish_reason"].as_str() {
                            return Some(Ok(StreamEvent::MessageDelta {
                                stop_reason: Some(reason.to_string()),
                                usage: None,
                            }));
                        }

                        if let Some(content) = choices[0]["delta"]["content"].as_str()
                            && !content.is_empty()
                        {
                            return Some(Ok(StreamEvent::TextDelta {
                                text: content.to_string(),
                            }));
                        }
                    }

                    None // skip chunks with no useful content
                }
                Err(e) => Some(Err(InfiniError::Stream(e.to_string()))),
            }
        });

        Ok(Box::pin(event_stream))
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_openai_url;

    #[test]
    fn bare_root_gets_full_path() {
        assert_eq!(
            normalize_openai_url("https://api.openai.com"),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn v1_suffix_appends_chat_completions() {
        assert_eq!(
            normalize_openai_url("https://api.openai.com/v1"),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn already_full_url_is_unchanged() {
        assert_eq!(
            normalize_openai_url("https://api.openai.com/v1/chat/completions"),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn dashscope_bare_root() {
        assert_eq!(
            normalize_openai_url("https://dashscope.aliyuncs.com/compatible-mode"),
            "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions"
        );
    }

    #[test]
    fn dashscope_with_v1() {
        assert_eq!(
            normalize_openai_url("https://dashscope.aliyuncs.com/compatible-mode/v1"),
            "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions"
        );
    }

    /// Regression: documents the current behaviour when a non-v1 path is passed.
    /// The contract is that `base_url` must be a root URL; this test makes the
    /// limitation visible so it doesn't regress silently.
    #[test]
    fn nonstandard_path_documents_known_limitation() {
        // /v2 is not a recognised suffix → /v1/chat/completions is appended.
        assert_eq!(
            normalize_openai_url("https://example.com/v2"),
            "https://example.com/v2/v1/chat/completions"
        );
    }
}
