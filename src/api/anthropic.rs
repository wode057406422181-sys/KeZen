use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue};
use secrecy::ExposeSecret;
use serde_json::json;

use crate::api::debug_logger;
use crate::api::types::{ContentBlock, Message, Role, StreamEvent, Usage};
use crate::api::{BoxStream, CacheHints, LlmClient, StreamOptions};
use crate::config::AppConfig;
use crate::constants::api::{ANTHROPIC_VERSION, CONTENT_TYPE_JSON};
use crate::error::KezenError;

/// Anthropic Messages API streaming client.
pub struct AnthropicClient {
    client: reqwest::Client,
    model: String,
    max_tokens: u32,
    base_url: String,
}

impl AnthropicClient {
    pub fn new(config: &AppConfig) -> Result<Self, KezenError> {
        let api_key = config.api_key.as_ref().map(|s| s.expose_secret().to_string()).ok_or(KezenError::NoApiKey)?;
        let model = config
            .model
            .as_deref()
            .ok_or(KezenError::NoModel)?
            .to_string();

        let mut headers = HeaderMap::new();
        headers.insert(
            "x-api-key",
            HeaderValue::from_str(&api_key)
                .map_err(|e| KezenError::Config(format!("Invalid API key format: {}", e)))?,
        );
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {}", api_key))
                .map_err(|e| KezenError::Config(format!("Invalid API key format: {}", e)))?,
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
                .map_err(|e| KezenError::Config(format!("Invalid User-Agent format: {}", e)))?,
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()?;

        let base_url = config.base_url().trim_end_matches('/').to_string();

        Ok(Self {
            client,
            model,
            max_tokens: config.max_tokens(),
            base_url,
        })
    }
}

/// Normalise an Anthropic-compatible base URL to the messages endpoint.
///
/// Accepts any of:
/// - bare root: `https://api.anthropic.com`
/// - with `/v1`: `https://api.anthropic.com/v1`
/// - already full: `https://api.anthropic.com/v1/messages`
pub(crate) fn normalize_anthropic_url(base: &str) -> String {
    if base.ends_with("/v1/messages") {
        base.to_string()
    } else if base.ends_with("/v1") {
        format!("{}/messages", base)
    } else {
        format!("{}/v1/messages", base)
    }
}

#[async_trait]
impl LlmClient for AnthropicClient {
    async fn stream(
        &self,
        messages: &[Message],
        system_prompt: Option<&str>,
        tools: Option<&[serde_json::Value]>,
        _options: &StreamOptions,
        cache_hints: Option<&CacheHints>,
        max_tokens_override: Option<u32>,
    ) -> Result<BoxStream<'_, StreamEvent>, KezenError> {
        let url = normalize_anthropic_url(&self.base_url);

        // Strip Thinking blocks from history: Anthropic's Messages API requires
        // thinking blocks in multi-turn history to carry a `signature` field
        // (extended-thinking beta verification). We don't retain signatures, so
        // sending them causes a 400. Text-only content is always safe.
        let cleaned_messages: Vec<serde_json::Value> = messages
            .iter()
            .map(|msg| {
                let role_str = match msg.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                    Role::System => "system",
                };
                let content: Vec<serde_json::Value> = msg
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Thinking { .. } => None, // drop — no signature
                        ContentBlock::Text { text } => {
                            Some(serde_json::json!({"type": "text", "text": text}))
                        }
                        ContentBlock::ToolUse { id, name, input } => Some(serde_json::json!({
                            "type": "tool_use",
                            "id": id,
                            "name": name,
                            "input": input,
                        })),
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content: result_content,
                            is_error,
                        } => Some(serde_json::json!({
                            "type": "tool_result",
                            "tool_use_id": tool_use_id,
                            "content": result_content,
                            "is_error": is_error,
                        })),
                    })
                    .collect();
                serde_json::json!({"role": role_str, "content": content})
            })
            .collect();

        let effective_max_tokens = max_tokens_override.unwrap_or(self.max_tokens);

        let mut body = json!({
            "model": self.model,
            "max_tokens": effective_max_tokens,
            "messages": cleaned_messages,
            "stream": true,
        });

        if let Some(sys_prompt) = system_prompt {
            if let Some(hints) = cache_hints
                && hints.cache_system
            {
                body["system"] = json!([{
                    "type": "text",
                    "text": sys_prompt,
                    "cache_control": {"type": "ephemeral"}
                }]);
            } else {
                body["system"] = json!(sys_prompt);
            }
        }

        let mut transformed_tools = None;
        if let Some(t) = tools {
            if !t.is_empty() {
                let mut tools_vec = t.to_vec();
                if let Some(hints) = cache_hints
                    && hints.cache_tools
                {
                    if let Some(last) = tools_vec.last_mut() {
                        if let serde_json::Value::Object(map) = last {
                            map.insert("cache_control".to_string(), json!({"type": "ephemeral"}));
                        }
                    }
                }
                transformed_tools = Some(tools_vec);
            }
        }

        if let Some(t_vec) = transformed_tools {
            body["tools"] = json!(t_vec);
            // Default to `auto` tool choice unless otherwise constrained
            body["tool_choice"] = json!({"type": "auto"});
        }

        // TODO: Anthropic native web search / web fetch support.
        // When `options.enable_server_search` is true, inject server-side tools:
        //   tools[].push({"type": "web_search_20250305", "name": "web_search", "max_uses": 5})
        //   tools[].push({"type": "web_fetch_20250910",  "name": "web_fetch",  "max_uses": 10})
        // And extend the SSE parser to handle:
        //   - "server_tool_use" content block type
        //   - "web_search_tool_result" content block type
        //   - "web_fetch_tool_result" content block type
        // See: https://platform.claude.com/docs/en/agents-and-tools/tool-use/web-search-tool
        //      https://platform.claude.com/docs/en/agents-and-tools/tool-use/web-fetch-tool

        debug_logger::log_request("anthropic", &url, &body);

        let response = self.client.post(&url).json(&body).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            debug_logger::log_error_response("anthropic", status.as_u16(), &text);
            tracing::error!(status = status.as_u16(), "Anthropic API error");
            return Err(KezenError::Api(format!(
                "Anthropic API error {}: {}",
                status, text
            )));
        }

        let stream = response.bytes_stream().eventsource();

        // Transform raw SSE events into typed StreamEvents.
        // Tool-use blocks are split at this layer: content_block_start with
        // type "tool_use" emits ToolUseStart, and input_json_delta chunks
        // emit ToolUseInputDelta, so the engine doesn't need to track block types.
        let event_stream = stream.filter_map(|event_result| async {
            match event_result {
                Ok(event) => {
                    debug_logger::log_sse_event("anthropic", &event.event, &event.data);
                    let parsed = match event.event.as_str() {
                        "message_start" => {
                            let v: serde_json::Value = match serde_json::from_str(&event.data) {
                                Ok(v) => v,
                                Err(e) => {
                                    return Some(Err(KezenError::Json(e)));
                                }
                            };
                            let role = match v["message"]["role"].as_str() {
                                Some("user") => Role::User,
                                _ => Role::Assistant,
                            };
                            let usage = Usage {
                                input_tokens: v["message"]["usage"]["input_tokens"]
                                    .as_u64()
                                    .unwrap_or(0),
                                output_tokens: v["message"]["usage"]["output_tokens"]
                                    .as_u64()
                                    .unwrap_or(0),
                                cache_creation_input_tokens: v["message"]["usage"]["cache_creation_input_tokens"]
                                    .as_u64()
                                    .unwrap_or(0),
                                cache_read_input_tokens: v["message"]["usage"]["cache_read_input_tokens"]
                                    .as_u64()
                                    .unwrap_or(0),
                            };
                            Ok(StreamEvent::MessageStart {
                                role,
                                usage: Some(usage),
                            })
                        }
                        "content_block_start" => {
                            let v: serde_json::Value = match serde_json::from_str(&event.data) {
                                Ok(v) => v,
                                Err(e) => {
                                    tracing::warn!(error = %e, "Anthropic: failed to parse SSE JSON");
                                    return Some(Err(KezenError::Json(e)));
                                }
                            };
                            let index = v["index"].as_u64().unwrap_or(0) as usize;
                            let block_type = v["content_block"]["type"]
                                .as_str()
                                .unwrap_or("text")
                                .to_string();

                            if block_type == "tool_use" {
                                let id = v["content_block"]["id"].as_str().unwrap_or("").to_string();
                                let name = v["content_block"]["name"].as_str().unwrap_or("").to_string();
                                return Some(Ok(StreamEvent::ToolUseStart { id, name }));
                            }

                            Ok(StreamEvent::ContentBlockStart { index, block_type })
                        }
                        "content_block_delta" => {
                            let v: serde_json::Value = match serde_json::from_str(&event.data) {
                                Ok(v) => v,
                                Err(e) => {
                                    tracing::warn!(error = %e, "Anthropic: failed to parse SSE JSON");
                                    return Some(Err(KezenError::Json(e)));
                                }
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
                                "input_json_delta" => {
                                    let text = v["delta"]["partial_json"].as_str().unwrap_or("").to_string();
                                    Ok(StreamEvent::ToolUseInputDelta { text })
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
                                Err(e) => {
                                    tracing::warn!(error = %e, "Anthropic: failed to parse SSE JSON");
                                    return Some(Err(KezenError::Json(e)));
                                }
                            };
                            let index = v["index"].as_u64().unwrap_or(0) as usize;
                            // Instead of ContentBlockStop for everything, emit ContentBlockStop so the engine
                            // knows we reached the end of the block. If Engine was in "Assemble Tool Input" state,
                            // It will treat it as ToolUseInputDone. Since we don't have block_type here,
                            // ContentBlockStop is the safest protocol mapping.
                            Ok(StreamEvent::ContentBlockStop { index })
                        }
                        "message_delta" => {
                            let v: serde_json::Value = match serde_json::from_str(&event.data) {
                                Ok(v) => v,
                                Err(e) => {
                                    tracing::warn!(error = %e, "Anthropic: failed to parse SSE JSON");
                                    return Some(Err(KezenError::Json(e)));
                                }
                            };
                            let stop_reason =
                                v["delta"]["stop_reason"].as_str().map(|s| s.to_string());
                            let usage = if v["usage"].is_object() {
                                Some(Usage {
                                    input_tokens: v["usage"]["input_tokens"].as_u64().unwrap_or(0),
                                    output_tokens: v["usage"]["output_tokens"].as_u64().unwrap_or(0),
                                    cache_creation_input_tokens: v["usage"]["cache_creation_input_tokens"]
                                        .as_u64()
                                        .unwrap_or(0),
                                    cache_read_input_tokens: v["usage"]["cache_read_input_tokens"]
                                        .as_u64()
                                        .unwrap_or(0),
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
                            Err(KezenError::Api(format!("Anthropic stream error: {}", v)))
                        }
                        _ => {
                            return None; // skip ping and unknown events
                        }
                    };
                    Some(parsed)
                }
                Err(e) => {
                    Some(Err(KezenError::Stream(e.to_string())))
                }
            }
        });

        Ok(Box::pin(event_stream))
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_anthropic_url;

    #[test]
    fn bare_root_gets_v1_messages() {
        assert_eq!(
            normalize_anthropic_url("https://api.anthropic.com"),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn v1_suffix_appends_messages() {
        assert_eq!(
            normalize_anthropic_url("https://api.anthropic.com/v1"),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn already_full_url_is_unchanged() {
        assert_eq!(
            normalize_anthropic_url("https://api.anthropic.com/v1/messages"),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn custom_proxy_bare_root() {
        assert_eq!(
            normalize_anthropic_url("https://my-proxy.internal"),
            "https://my-proxy.internal/v1/messages"
        );
    }

    #[test]
    fn custom_proxy_with_v1() {
        assert_eq!(
            normalize_anthropic_url("https://my-proxy.internal/v1"),
            "https://my-proxy.internal/v1/messages"
        );
    }
}
