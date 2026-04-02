pub mod events;
pub mod session;

use futures::StreamExt;
use tokio::sync::mpsc;

use crate::api::types::{ContentBlock, Message, Role, StreamEvent, Usage};
use crate::api::{LlmClient, create_client};
use crate::config::AppConfig;
use crate::prompts::build_system_prompt;

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
    action_rx: mpsc::Receiver<UserAction>,
    event_tx: mpsc::Sender<EngineEvent>,
}

impl InfiniEngine {
    pub fn new(
        config: AppConfig,
        action_rx: mpsc::Receiver<UserAction>,
        event_tx: mpsc::Sender<EngineEvent>,
    ) -> Result<Self, crate::error::InfiniError> {
        let client = create_client(&config)?;
        Ok(Self {
            config,
            client,
            session: Session::new(),
            action_rx,
            event_tx,
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
        // Add user message to session
        self.session.add_message(Message {
            role: Role::User,
            content: vec![ContentBlock::Text { text: content }],
        });

        let system_prompt = build_system_prompt();
        // Clone messages to avoid borrow conflict with session
        let messages = self.session.messages();

        let stream_result = self.client.stream(&messages, Some(&system_prompt)).await;

        match stream_result {
            Ok(mut stream) => {
                let mut assistant_text = String::new();
                let mut thinking_text = String::new();
                let mut turn_usage = Usage::default();

                loop {
                    tokio::select! {
                        biased;

                        // Check for cancel actions first
                        cancel_action = self.action_rx.recv() => {
                            if let Some(UserAction::Cancel) = cancel_action {
                                break;
                            }
                            // Other actions during streaming are ignored
                        }

                        evt_opt = stream.next() => {
                            match evt_opt {
                                Some(Ok(evt)) => {
                                    match evt {
                                        StreamEvent::TextDelta { text } => {
                                            assistant_text.push_str(&text);
                                            let _ = self.event_tx.send(
                                                EngineEvent::TextDelta { text }
                                            ).await;
                                        }
                                        StreamEvent::ThinkingDelta { text } => {
                                            thinking_text.push_str(&text);
                                            let _ = self.event_tx.send(
                                                EngineEvent::ThinkingDelta { text }
                                            ).await;
                                        }
                                        StreamEvent::MessageStart { usage: Some(u), .. } => {
                                            turn_usage.input_tokens = u.input_tokens;
                                        }
                                        StreamEvent::MessageDelta { usage: Some(u), .. } => {
                                            // MessageDelta usage often has output_tokens
                                            if u.output_tokens > 0 {
                                                turn_usage.output_tokens = u.output_tokens;
                                            }
                                            if u.input_tokens > 0 {
                                                turn_usage.input_tokens = u.input_tokens;
                                            }
                                        }
                                        StreamEvent::MessageStop => {
                                            break;
                                        }
                                        _ => {} // ContentBlockStart/Stop handled implicitly
                                    }
                                }
                                Some(Err(e)) => {
                                    let _ = self.event_tx.send(
                                        EngineEvent::Error { message: e.to_string() }
                                    ).await;
                                    break;
                                }
                                None => {
                                    break; // Stream ended
                                }
                            }
                        }
                    }
                }

                // Update session with usage
                self.session.update_usage(&turn_usage);
                let _ = self
                    .event_tx
                    .send(EngineEvent::CostUpdate(turn_usage))
                    .await;

                // Build assistant content blocks
                let mut blocks = Vec::new();
                if !thinking_text.is_empty() {
                    blocks.push(ContentBlock::Thinking {
                        thinking: thinking_text,
                    });
                }
                if !assistant_text.is_empty() {
                    blocks.push(ContentBlock::Text {
                        text: assistant_text,
                    });
                }

                if !blocks.is_empty() {
                    self.session.add_message(Message {
                        role: Role::Assistant,
                        content: blocks,
                    });
                }

                let _ = self.event_tx.send(EngineEvent::Done).await;
            }
            Err(e) => {
                let _ = self
                    .event_tx
                    .send(EngineEvent::Error {
                        message: e.to_string(),
                    })
                    .await;
                let _ = self.event_tx.send(EngineEvent::Done).await;
            }
        }
    }
}
