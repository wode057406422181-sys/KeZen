pub mod events;
pub mod session;

use futures::StreamExt;
use tokio::sync::mpsc;

use crate::api::debug_logger;
use crate::api::types::{ContentBlock, Message, Role, StreamEvent, Usage};
use crate::api::{self, LlmClient};
use crate::config::AppConfig;
use crate::prompts::build_system_prompt;
use crate::tools::registry::ToolRegistry;

use self::events::{EngineEvent, UserAction};
use self::session::Session;

/// The core engine that orchestrates LLM interactions.
///
/// Engine communicates with frontends exclusively through channels:
/// - Receives `UserAction` from frontends via `action_rx`
/// - Sends `EngineEvent` to frontends via `event_tx`
///
/// **Invariant**: This module MUST NOT import std::io, crossterm, or rustyline.
pub struct InfiniEngine {
    #[allow(dead_code)]
    config: AppConfig,
    client: Box<dyn LlmClient>,
    session: Session,
    /// Cached system prompt, built once at construction to avoid repeated
    /// blocking I/O (e.g. `git rev-parse`) on the async runtime.
    system_prompt: String,
    action_rx: mpsc::Receiver<UserAction>,
    event_tx: mpsc::Sender<EngineEvent>,
    registry: ToolRegistry,
}

impl InfiniEngine {
    pub fn new(
        config: AppConfig,
        action_rx: mpsc::Receiver<UserAction>,
        event_tx: mpsc::Sender<EngineEvent>,
        registry: ToolRegistry,
    ) -> Result<Self, crate::error::InfiniError> {
        let client = api::create_client(&config)?;
        // Build the system prompt once at construction.
        // NOTE: `build_system_prompt` calls `std::process::Command` (blocking).
        // This runs on the tokio thread that calls `InfiniEngine::new()`; callers
        // should wrap this in `tokio::task::spawn_blocking` if latency is critical.
        let system_prompt = build_system_prompt(config.model.as_deref());
        Ok(Self {
            config,
            client,
            session: Session::new(),
            system_prompt,
            action_rx,
            event_tx,
            registry,
        })
    }

    /// Main engine loop. Runs until the action channel is closed.
    pub async fn run(mut self) {
        while let Some(action) = self.action_rx.recv().await {
            match action {
                UserAction::SendMessage { content } => {
                    self.handle_send_message(content).await;
                }
                UserAction::Cancel => {
                    // Ignored if not currently streaming
                }
            }
        }
    }

    /// Handle a user message: send to LLM, stream response, accumulate history.
    async fn handle_send_message(&mut self, content: String) {
        self.session.add_message(Message {
            role: Role::User,
            content: vec![ContentBlock::Text { text: content }],
        });

        let mut iterations = 0;
        let max_iterations = 25;

        // Agentic loop: keeps calling the LLM until it stops requesting tools
        // or we hit the safety cap.
        loop {
            if iterations >= max_iterations {
                let _ = self.event_tx.send(EngineEvent::Error { message: "Max tool loop iterations reached".into() }).await;
                break;
            }
            iterations += 1;

            let schemas = self.registry.schemas();
            let tools_arg = if schemas.is_empty() { None } else { Some(schemas.as_slice()) };

            let stream_result = self
                .client
                .stream(self.session.messages(), Some(&self.system_prompt), tools_arg)
                .await;

            let mut assistant_text = String::new();
            let mut thinking_text = String::new();
            let mut turn_usage = Usage::default();

            let mut pending_tools = Vec::new();
            // Tracks the tool currently being streamed: (id, name, accumulated_json_input)
            let mut current_tool: Option<(String, String, String)> = None;
            let mut stream_interrupted = false;

            match stream_result {
                Ok(mut stream) => {
                    loop {
                        // biased: check cancel before stream to ensure responsiveness
                        tokio::select! {
                            biased;

                            cancel_action = self.action_rx.recv() => {
                                match cancel_action {
                                    Some(UserAction::Cancel) | None => {
                                        stream_interrupted = true;
                                        break;
                                    }
                                    Some(_) => {}
                                }
                            }

                            evt_opt = stream.next() => {
                                match evt_opt {
                                    Some(Ok(evt)) => {
                                        match evt {
                                            StreamEvent::TextDelta { text } => {
                                                assistant_text.push_str(&text);
                                                let _ = self.event_tx.send(EngineEvent::TextDelta { text }).await;
                                            }
                                            StreamEvent::ThinkingDelta { text } => {
                                                thinking_text.push_str(&text);
                                                let _ = self.event_tx.send(EngineEvent::ThinkingDelta { text }).await;
                                            }
                                            StreamEvent::ToolUseStart { id, name } => {
                                                current_tool = Some((id, name, String::new()));
                                            }
                                            StreamEvent::ToolUseInputDelta { text } => {
                                                if let Some((_, _, ref mut input)) = current_tool {
                                                    input.push_str(&text);
                                                }
                                            }
                                            StreamEvent::ToolUseInputDone | StreamEvent::ContentBlockStop { .. } => {
                                                if let Some((id, name, input_str)) = current_tool.take() {
                                                    let input = serde_json::from_str(&if input_str.is_empty() { "{}".to_string() } else { input_str.clone() }).unwrap_or(serde_json::json!({}));
                                                    let _ = self.event_tx.send(EngineEvent::ToolUseStart { id: id.clone(), name: name.clone(), input: input.clone() }).await;
                                                    pending_tools.push((id, name, input));
                                                }
                                            }
                                            StreamEvent::MessageStart { usage: Some(u), .. } => {
                                                turn_usage.input_tokens = u.input_tokens;
                                            }
                                            StreamEvent::MessageDelta { usage, .. } => {
                                                if let Some(u) = usage {
                                                    if u.output_tokens > 0 { turn_usage.output_tokens = u.output_tokens; }
                                                    if u.input_tokens > 0 { turn_usage.input_tokens = u.input_tokens; }
                                                }
                                            }
                                            // Flush pending tool on MessageStop in case
                                            // ContentBlockStop was not emitted (some providers).
                                            StreamEvent::MessageStop => {
                                                if let Some((id, name, input_str)) = current_tool.take() {
                                                    let input = serde_json::from_str(&if input_str.is_empty() { "{}".to_string() } else { input_str.clone() }).unwrap_or(serde_json::json!({}));
                                                    let _ = self.event_tx.send(EngineEvent::ToolUseStart { id: id.clone(), name: name.clone(), input: input.clone() }).await;
                                                    pending_tools.push((id, name, input));
                                                }
                                                break;
                                            }
                                            // ContentBlockStart is intentionally not handled here.
                                            // For text blocks it carries no content (deltas arrive via TextDelta).
                                            // For tool blocks the id/name are already extracted as ToolUseStart
                                            // by the provider-level SSE parser (anthropic.rs / openai.rs).
                                            _ => {}
                                        }
                                    }
                                    Some(Err(e)) => {
                                        let _ = self.event_tx.send(EngineEvent::Error { message: e.to_string() }).await;
                                        stream_interrupted = true;
                                        break;
                                    }
                                    None => {
                                        break;
                                    }
                                }
                            }
                        }
                    }

                    self.session.update_usage(&turn_usage);
                    debug_logger::log_stream_end(
                        &self.config.provider.to_string(),
                        turn_usage.input_tokens,
                        turn_usage.output_tokens,
                    );
                    let _ = self.event_tx.send(EngineEvent::CostUpdate(turn_usage)).await;

                    if stream_interrupted {
                        let _ = self.event_tx.send(EngineEvent::Done).await;
                        break;
                    }

                    // Record this turn's assistant response in conversation history.
                    let mut blocks = Vec::new();
                    if !thinking_text.is_empty() {
                        blocks.push(ContentBlock::Thinking { thinking: thinking_text });
                    }
                    if !assistant_text.is_empty() {
                        blocks.push(ContentBlock::Text { text: assistant_text });
                    }
                    for (id, name, input) in &pending_tools {
                        blocks.push(ContentBlock::ToolUse { id: id.clone(), name: name.clone(), input: input.clone() });
                    }
                    
                    if !blocks.is_empty() {
                        self.session.add_message(Message { role: Role::Assistant, content: blocks });
                    }

                    // No tools requested: the LLM is done, exit the loop.
                    if pending_tools.is_empty() {
                        let _ = self.event_tx.send(EngineEvent::Done).await;
                        break;
                    }

                    // Execute each requested tool and collect results.
                    let mut tool_results = Vec::new();
                    for (id, name, input) in pending_tools {
                        let result = match self.registry.get(&name) {
                            Some(tool) => tool.call(input).await,
                            None => crate::tools::ToolResult {
                                content: format!("Tool {} not found", name),
                                is_error: true,
                            }
                        };
                        
                        let _ = self.event_tx.send(EngineEvent::ToolResult {
                            id: id.clone(),
                            output: result.content.clone(),
                            is_error: result.is_error,
                        }).await;
                        
                        tool_results.push(ContentBlock::ToolResult {
                            tool_use_id: id,
                            content: result.content,
                            is_error: result.is_error,
                        });
                    }

                    // Feed tool results back as a "user" message so the LLM
                    // can see the outputs and decide the next step.
                    self.session.add_message(Message { role: Role::User, content: tool_results });
                }
                Err(e) => {
                    let _ = self.event_tx.send(EngineEvent::Error { message: e.to_string() }).await;
                    let _ = self.event_tx.send(EngineEvent::Done).await;
                    break;
                }
            }
        }
    }
}

