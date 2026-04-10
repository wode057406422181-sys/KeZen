use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::ReceiverStream;

use super::kezen_proto::{ClientMessage, ServerMessage, kezen_agent_client::KezenAgentClient};
use crate::engine::events::{EngineEvent, UserAction};
pub async fn run_grpc_client(
    url: String,
    mut action_rx: mpsc::Receiver<UserAction>,
    event_tx: broadcast::Sender<EngineEvent>,
) -> anyhow::Result<()> {
    // 1. Connect to server
    let mut client = KezenAgentClient::connect(url.clone())
        .await
        .map_err(|e| anyhow::anyhow!("Failed to connect to {}: {}", url, e))?;

    // 2. Setup outbound stream (REPL -> Server)
    let (outbound_tx, outbound_rx) = mpsc::channel(100);
    let request_stream = ReceiverStream::new(outbound_rx);

    // Create initial hello handshake
    let hello = ClientMessage {
        action: Some(super::kezen_proto::client_message::Action::Hello(
            super::kezen_proto::Hello {
                protocol_version: 1,
                client_name: "kezen-client".to_string(),
            },
        )),
    };
    outbound_tx.send(hello).await?;

    // 3. Forward REPL actions to gRPC Outbound
    let out_tx_clone = outbound_tx.clone();
    drop(outbound_tx); // Drop the original sender so the stream completes when task finishes
    let action_task = tokio::spawn(async move {
        while let Some(action) = action_rx.recv().await {
            if let Some(msg) = user_action_to_client_message(action) {
                if out_tx_clone.send(msg).await.is_err() {
                    break;
                }
            }
        }
    });

    // 4. Send request and get response stream
    let response = client
        .stream_session(tonic::Request::new(request_stream))
        .await;

    let res = match response {
        Ok(res) => {
            let mut inbound_stream = res.into_inner();
            while let Ok(Some(server_msg)) = inbound_stream.message().await {
                if let Some(event) = server_message_to_engine_event(server_msg) {
                    let _ = event_tx.send(event);
                }
            }
            let _ = event_tx.send(EngineEvent::Error {
                message: "Server disconnected gracefully".to_string(),
            });
            Ok(())
        }
        Err(e) => {
            let _ = event_tx.send(EngineEvent::Error {
                message: format!("Lost connection to server: {}", e),
            });
            Err(anyhow::anyhow!("gRPC session error: {}", e))
        }
    };

    action_task.abort();
    res
}

fn user_action_to_client_message(action: UserAction) -> Option<ClientMessage> {
    match action {
        UserAction::SendMessage { content } => Some(ClientMessage {
            action: Some(super::kezen_proto::client_message::Action::SendMessage(
                super::kezen_proto::SendMessage { content },
            )),
        }),
        UserAction::Cancel => Some(ClientMessage {
            action: Some(super::kezen_proto::client_message::Action::Cancel(
                super::kezen_proto::Cancel {},
            )),
        }),
        UserAction::PermissionResponse {
            id,
            allowed,
            always_allow,
        } => {
            let decision = if allowed {
                if always_allow {
                    Some(
                        super::kezen_proto::permission_response::Decision::AlwaysAllow(
                            super::kezen_proto::AlwaysAllow {
                                suggestion_index: 0,
                            }, // Re-check this if you use suggestion_index later
                        ),
                    )
                } else {
                    Some(
                        super::kezen_proto::permission_response::Decision::AllowOnce(
                            super::kezen_proto::AllowOnce {},
                        ),
                    )
                }
            } else {
                Some(super::kezen_proto::permission_response::Decision::Deny(
                    super::kezen_proto::Deny { reason: None },
                ))
            };
            Some(ClientMessage {
                action: Some(
                    super::kezen_proto::client_message::Action::PermissionResponse(
                        super::kezen_proto::PermissionResponse {
                            request_id: id,
                            decision,
                        },
                    ),
                ),
            })
        }
    }
}

fn server_message_to_engine_event(msg: ServerMessage) -> Option<EngineEvent> {
    use super::kezen_proto::server_message::Event;
    match msg.event {
        Some(Event::TextDelta(t)) => Some(EngineEvent::TextDelta { text: t.text }),
        Some(Event::ThinkingDelta(t)) => Some(EngineEvent::ThinkingDelta { text: t.text }),
        Some(Event::CostUpdate(c)) => {
            if let Some(usage) = c.usage {
                Some(EngineEvent::CostUpdate(crate::api::types::Usage {
                    input_tokens: usage.input_tokens,
                    output_tokens: usage.output_tokens,
                    cache_creation_input_tokens: usage.cache_creation_input_tokens,
                    cache_read_input_tokens: usage.cache_read_input_tokens,
                }))
            } else {
                None
            }
        }
        Some(Event::ToolUseStart(t)) => {
            let input = serde_json::from_str(&t.input_json).unwrap_or(serde_json::json!({}));
            Some(EngineEvent::ToolUseStart {
                id: t.tool_use_id,
                name: t.name,
                input,
            })
        }
        Some(Event::ToolResult(t)) => Some(EngineEvent::ToolResult {
            id: t.tool_use_id,
            output: t.output,
            is_error: t.is_error,
        }),
        Some(Event::PermissionRequest(p)) => {
            let risk = match p.risk_level {
                1 => crate::permissions::RiskLevel::Low,
                2 => crate::permissions::RiskLevel::Medium,
                3 => crate::permissions::RiskLevel::High,
                _ => crate::permissions::RiskLevel::Low,
            };
            Some(EngineEvent::PermissionRequest {
                id: p.request_id,
                tool: p.tool,
                description: p.description,
                risk_level: risk,
                suggestion: p.suggestions.into_iter().next(),
            })
        }
        Some(Event::SlashCommandResult(s)) => Some(EngineEvent::SlashCommandResult {
            command: s.command,
            output: s.output,
        }),
        Some(Event::CompactProgress(c)) => {
            Some(EngineEvent::CompactProgress { message: c.message })
        }
        Some(Event::SkillLoaded(s)) => Some(EngineEvent::SkillLoaded { name: s.name }),
        Some(Event::Done(_)) => Some(EngineEvent::Done),
        Some(Event::Error(e)) => Some(EngineEvent::Error { message: e.message }),
        Some(Event::Warning(w)) => Some(EngineEvent::Warning(w.message)),
        Some(Event::SessionRestored(sr)) => {
            let messages = serde_json::from_str(&sr.messages_json).unwrap_or_default();
            Some(EngineEvent::SessionRestored { messages })
        }
        Some(Event::ServerHello(_)) => None, // Just handshake logging
        None => None,
    }
}
