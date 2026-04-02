use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::json;

use crate::api::debug_logger;
use crate::api::types::{Message, Role, StreamEvent, Usage};
use crate::api::{BoxStream, LlmClient};
use crate::config::AppConfig;
use crate::constants::api::{ANTHROPIC_VERSION, CONTENT_TYPE_JSON};
use crate::constants::defaults::DEFAULT_MAX_TOKENS;
use crate::error::InfiniError;

pub struct AnthropicClient {
    client: reqwest::Client,
    model: String,
    max_tokens: u32,
    base_url: String,
}

impl AnthropicClient {
    pub fn new(config: &AppConfig) -> Result<Self, InfiniError> {
        let api_key = config.api_key.as_deref().ok_or(InfiniError::NoApiKey)?;
        let model = config
            .model
            .as_deref()
            .ok_or(InfiniError::NoModel)?
            .to_string();

        let mut headers = HeaderMap::new();
        headers.insert(
            "x-api-key",
            HeaderValue::from_str(api_key)
                .map_err(|e| InfiniError::Config(format!("Invalid API key format: {}", e)))?,
        );
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {}", api_key))
                .map_err(|e| InfiniError::Config(format!("Invalid API key format: {}", e)))?,
        );
        headers.insert(
            "anthropic-version",
            HeaderValue::from_static(ANTHROPIC_VERSION),
        );
        headers.insert("content-type", HeaderValue::from_static(CONTENT_TYPE_JSON));
        // Identify as a coding agent (required by some Anthropic-compatible endpoints)
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
        })
    }
}

#[async_trait]
impl LlmClient for AnthropicClient {
    async fn stream(
        &self,
        messages: &[Message],
        system_prompt: Option<&str>,
    ) -> Result<BoxStream<'_, StreamEvent>, InfiniError> {
        let url = if self.base_url.ends_with("/v1/messages") {
            self.base_url.clone()
        } else if self.base_url.ends_with("/v1") {
            format!("{}/messages", self.base_url)
        } else {
            format!("{}/v1/messages", self.base_url)
        };

        let mut body = json!({
            "model": self.model,
            "max_tokens": self.max_tokens,
            "messages": messages,
            "stream": true,
        });

        if let Some(sys_prompt) = system_prompt {
            body["system"] = json!(sys_prompt);
        }

        debug_logger::log_request("anthropic", &url, &body);

        let response = self.client.post(&url).json(&body).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            debug_logger::log_error_response("anthropic", status.as_u16(), &text);
            return Err(InfiniError::Api(format!(
                "Anthropic API error {}: {}",
                status, text
            )));
        }

        let stream = response.bytes_stream().eventsource();

        // Track current content block type to properly route deltas
        let event_stream = stream.filter_map(|event_result| async {
            match event_result {
                Ok(event) => {
                    debug_logger::log_sse_event("anthropic", &event.event, &event.data);
                    let parsed = match event.event.as_str() {
                        "message_start" => {
                            let v: serde_json::Value = match serde_json::from_str(&event.data) {
                                Ok(v) => v,
                                Err(e) => return Some(Err(InfiniError::Json(e))),
                            };
                            let role = match v["message"]["role"].as_str() {
                                Some("user") => Role::User,
                                _ => Role::Assistant,
                            };
                            let usage = Usage {
                                input_tokens: v["message"]["usage"]["input_tokens"]
                                    .as_u64()
                                    .unwrap_or(0)
                                    as u32,
                                output_tokens: v["message"]["usage"]["output_tokens"]
                                    .as_u64()
                                    .unwrap_or(0)
                                    as u32,
                            };
                            Ok(StreamEvent::MessageStart {
                                role,
                                usage: Some(usage),
                            })
                        }
                        "content_block_start" => {
                            let v: serde_json::Value = match serde_json::from_str(&event.data) {
                                Ok(v) => v,
                                Err(e) => return Some(Err(InfiniError::Json(e))),
                            };
                            let index = v["index"].as_u64().unwrap_or(0) as usize;
                            let block_type = v["content_block"]["type"]
                                .as_str()
                                .unwrap_or("text")
                                .to_string();
                            Ok(StreamEvent::ContentBlockStart { index, block_type })
                        }
                        "content_block_delta" => {
                            let v: serde_json::Value = match serde_json::from_str(&event.data) {
                                Ok(v) => v,
                                Err(e) => return Some(Err(InfiniError::Json(e))),
                            };
                            let delta_type = v["delta"]["type"].as_str().unwrap_or("");
                            match delta_type {
                                "thinking_delta" => {
                                    let text =
                                        v["delta"]["thinking"].as_str().unwrap_or("").to_string();
                                    if text.is_empty() {
                                        return None; // skip empty deltas
                                    }
                                    Ok(StreamEvent::ThinkingDelta { text })
                                }
                                _ => {
                                    // text_delta or unknown
                                    let text =
                                        v["delta"]["text"].as_str().unwrap_or("").to_string();
                                    if text.is_empty() {
                                        return None; // skip empty deltas
                                    }
                                    Ok(StreamEvent::TextDelta { text })
                                }
                            }
                        }
                        "content_block_stop" => {
                            let v: serde_json::Value = match serde_json::from_str(&event.data) {
                                Ok(v) => v,
                                Err(e) => return Some(Err(InfiniError::Json(e))),
                            };
                            let index = v["index"].as_u64().unwrap_or(0) as usize;
                            Ok(StreamEvent::ContentBlockStop { index })
                        }
                        "message_delta" => {
                            let v: serde_json::Value = match serde_json::from_str(&event.data) {
                                Ok(v) => v,
                                Err(e) => return Some(Err(InfiniError::Json(e))),
                            };
                            let stop_reason =
                                v["delta"]["stop_reason"].as_str().map(|s| s.to_string());
                            let usage = if v["usage"].is_object() {
                                Some(Usage {
                                    input_tokens: v["usage"]["input_tokens"].as_u64().unwrap_or(0)
                                        as u32,
                                    output_tokens: v["usage"]["output_tokens"].as_u64().unwrap_or(0)
                                        as u32,
                                })
                            } else {
                                None
                            };
                            Ok(StreamEvent::MessageDelta { stop_reason, usage })
                        }
                        "message_stop" => Ok(StreamEvent::MessageStop),
                        "error" => {
                            let v: serde_json::Value =
                                serde_json::from_str(&event.data).unwrap_or_default();
                            Err(InfiniError::Api(format!("Anthropic stream error: {}", v)))
                        }
                        _ => {
                            return None; // skip ping and unknown events
                        }
                    };
                    Some(parsed)
                }
                Err(e) => Some(Err(InfiniError::Stream(e.to_string()))),
            }
        });

        Ok(Box::pin(event_stream))
    }
}
