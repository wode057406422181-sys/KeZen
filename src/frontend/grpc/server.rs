use std::net::SocketAddr;
use tokio::sync::{broadcast, mpsc};

use crate::engine::events::{EngineEvent, UserAction};

use super::kezen_proto;

use super::kezen_proto::{
    kezen_agent_server::{KezenAgent, KezenAgentServer},
    ClientMessage, ServerMessage,
};

pub struct KezenAgentService {
    action_tx: mpsc::Sender<UserAction>,
    event_tx: broadcast::Sender<EngineEvent>,
}

#[tonic::async_trait]
impl KezenAgent for KezenAgentService {
    type StreamSessionStream = tokio_stream::wrappers::ReceiverStream<Result<ServerMessage, tonic::Status>>;

    async fn stream_session(
        &self,
        request: tonic::Request<tonic::Streaming<ClientMessage>>,
    ) -> Result<tonic::Response<Self::StreamSessionStream>, tonic::Status> {
        let mut in_stream = request.into_inner();
        let (out_tx, out_rx) = tokio::sync::mpsc::channel(100);
        
        let action_tx = self.action_tx.clone();
        let mut event_rx = self.event_tx.subscribe();

        // Task 1: Engine -> gRPC Client
        tokio::spawn(async move {
            // Send the ServerHello handshake (m-1)
            let hello = ServerMessage {
                event: Some(kezen_proto::server_message::Event::ServerHello(kezen_proto::ServerHello {
                    protocol_version: 1,
                    server_version: env!("CARGO_PKG_VERSION").to_string(),
                })),
            };
            if out_tx.send(Ok(hello)).await.is_err() {
                return;
            }

            loop {
                match event_rx.recv().await {
                    Ok(event) => {
                        let msg = engine_event_to_server_message(event);
                        if out_tx.send(Ok(msg)).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(skipped = n, "gRPC client lagged, dropped events");
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        // Task 2: gRPC Client -> Engine
        tokio::spawn(async move {
            loop {
                match in_stream.message().await {
                    Ok(Some(msg)) => {
                        if let Some(action) = client_message_to_user_action(msg) {
                            let _ = action_tx.send(action).await;
                        }
                    }
                    Ok(None) => break, // client closed stream gracefully
                    Err(status) => {
                        tracing::warn!(%status, "gRPC client stream error");
                        break;
                    }
                }
            }
        });

        Ok(tonic::Response::new(tokio_stream::wrappers::ReceiverStream::new(out_rx)))
    }
}

pub async fn start_grpc_server(
    addr: SocketAddr,
    action_tx: mpsc::Sender<UserAction>,
    event_tx: broadcast::Sender<EngineEvent>,
) -> anyhow::Result<()> {
    tracing::info!(%addr, "Starting KeZen gRPC server");
    
    let service = KezenAgentService { action_tx, event_tx };
    
    tonic::transport::Server::builder()
        .add_service(KezenAgentServer::new(service))
        .serve(addr)
        .await?;
        
    Ok(())
}

fn client_message_to_user_action(msg: ClientMessage) -> Option<UserAction> {
    use kezen_proto::client_message::Action;
    match msg.action? {
        Action::Hello(_) => None, // Engine doesn't process Hello currently
        Action::SendMessage(sm) => Some(UserAction::SendMessage { content: sm.content }),
        Action::Cancel(_) => Some(UserAction::Cancel),
        Action::PermissionResponse(pr) => {
            use kezen_proto::permission_response::Decision;
            let (allowed, always_allow) = match pr.decision {
                Some(Decision::Deny(_)) => (false, false),
                Some(Decision::AllowOnce(_)) => (true, false),
                Some(Decision::AlwaysAllow(_)) => {
                    // TODO: pass suggestion_index to Engine when UserAction is extended
                    (true, true)
                }
                None => (false, false),
            };
            Some(UserAction::PermissionResponse {
                id: pr.request_id,
                allowed,
                always_allow,
            })
        }
    }
}

fn engine_event_to_server_message(event: EngineEvent) -> ServerMessage {
    use kezen_proto::server_message::Event;
    
    let ev = match event {
        EngineEvent::TextDelta { text } => Event::TextDelta(kezen_proto::TextDelta { text }),
        EngineEvent::ThinkingDelta { text } => Event::ThinkingDelta(kezen_proto::ThinkingDelta { text }),
        EngineEvent::CostUpdate(usage) => Event::CostUpdate(kezen_proto::CostUpdate {
            usage: Some(kezen_proto::TokenUsage {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                cache_creation_input_tokens: usage.cache_creation_input_tokens,
                cache_read_input_tokens: usage.cache_read_input_tokens,
            }),
        }),
        EngineEvent::ToolUseStart { id, name, input } => Event::ToolUseStart(kezen_proto::ToolUseStart {
            tool_use_id: id,
            name,
            input_json: serde_json::to_string(&input).unwrap_or_default(),
        }),
        EngineEvent::ToolResult { id, output, is_error } => Event::ToolResult(kezen_proto::ToolResult {
            tool_use_id: id,
            output,
            is_error,
        }),
        EngineEvent::PermissionRequest { id, tool, description, risk_level, suggestion } => {
            // Convert domain RiskLevel to Proto RiskLevel
            let proto_risk = match risk_level {
                crate::permissions::RiskLevel::Low => kezen_proto::RiskLevel::Low,
                crate::permissions::RiskLevel::Medium => kezen_proto::RiskLevel::Medium,
                crate::permissions::RiskLevel::High => kezen_proto::RiskLevel::High,
            };
            let mut suggestions = Vec::new();
            if let Some(s) = suggestion {
                suggestions.push(s);
            }
            Event::PermissionRequest(kezen_proto::PermissionRequest {
                request_id: id,
                tool_use_id: None, // Will be filled dynamically if tool context is available later
                tool,
                description,
                risk_level: proto_risk as i32,
                suggestions,
            })
        }
        EngineEvent::SlashCommandResult { command, output } => Event::SlashCommandResult(kezen_proto::SlashCommandResult {
            command,
            output,
        }),
        EngineEvent::CompactProgress { message } => Event::CompactProgress(kezen_proto::CompactProgress { message }),
        EngineEvent::SkillLoaded { name } => Event::SkillLoaded(kezen_proto::SkillLoaded { name }),
        EngineEvent::SessionRestored { messages } => {
            let json = serde_json::to_string(&messages).unwrap_or_else(|_| "[]".to_string());
            Event::SessionRestored(kezen_proto::SessionRestored { messages_json: json })
        }
        EngineEvent::Done => Event::Done(kezen_proto::Done {}),
        EngineEvent::Error { message } => Event::Error(kezen_proto::Error { message }),
        EngineEvent::Warning(message) => Event::Warning(kezen_proto::Warning { message }),
    };
    
    ServerMessage { event: Some(ev) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_message_to_user_action() {
        // SendMessage
        let msg = ClientMessage {
            action: Some(kezen_proto::client_message::Action::SendMessage(kezen_proto::SendMessage {
                content: "hello world".to_string(),
            })),
        };
        let action = client_message_to_user_action(msg).unwrap();
        assert_eq!(
            action,
            UserAction::SendMessage { content: "hello world".to_string() }
        );

        // Cancel
        let msg = ClientMessage {
            action: Some(kezen_proto::client_message::Action::Cancel(kezen_proto::Cancel {})),
        };
        let action = client_message_to_user_action(msg).unwrap();
        assert_eq!(action, UserAction::Cancel);

        // PermissionResponse - AllowOnce
        let msg = ClientMessage {
            action: Some(kezen_proto::client_message::Action::PermissionResponse(
                kezen_proto::PermissionResponse {
                    request_id: "req-123".to_string(),
                    decision: Some(kezen_proto::permission_response::Decision::AllowOnce(
                        kezen_proto::AllowOnce {},
                    )),
                },
            )),
        };
        let action = client_message_to_user_action(msg).unwrap();
        assert_eq!(
            action,
            UserAction::PermissionResponse {
                id: "req-123".to_string(),
                allowed: true,
                always_allow: false,
            }
        );
    }

    #[test]
    fn test_client_message_extended() {
        // Hello
        let msg = ClientMessage {
            action: Some(kezen_proto::client_message::Action::Hello(kezen_proto::Hello {
                protocol_version: 1,
                client_name: "test".to_string(),
            })),
        };
        assert_eq!(client_message_to_user_action(msg), None);

        // Deny
        let msg = ClientMessage {
            action: Some(kezen_proto::client_message::Action::PermissionResponse(
                kezen_proto::PermissionResponse {
                    request_id: "req-123".to_string(),
                    decision: Some(kezen_proto::permission_response::Decision::Deny(
                        kezen_proto::Deny { reason: None },
                    )),
                },
            )),
        };
        let action = client_message_to_user_action(msg).unwrap();
        assert_eq!(
            action,
            UserAction::PermissionResponse {
                id: "req-123".to_string(),
                allowed: false,
                always_allow: false,
            }
        );

        // AlwaysAllow
        let msg = ClientMessage {
            action: Some(kezen_proto::client_message::Action::PermissionResponse(
                kezen_proto::PermissionResponse {
                    request_id: "req-123".to_string(),
                    decision: Some(kezen_proto::permission_response::Decision::AlwaysAllow(
                        kezen_proto::AlwaysAllow { suggestion_index: 0 },
                    )),
                },
            )),
        };
        let action = client_message_to_user_action(msg).unwrap();
        assert_eq!(
            action,
            UserAction::PermissionResponse {
                id: "req-123".to_string(),
                allowed: true,
                always_allow: true,
            }
        );
    }

    #[test]
    fn test_engine_event_to_server_message() {
        // TextDelta
        let event = EngineEvent::TextDelta { text: "response delta".to_string() };
        let msg = engine_event_to_server_message(event);
        match msg.event {
            Some(kezen_proto::server_message::Event::TextDelta(t)) => assert_eq!(t.text, "response delta"),
            _ => panic!("Expected TextDelta variant"),
        }
        
        // ThinkingDelta
        let event = EngineEvent::ThinkingDelta { text: "thinking".to_string() };
        let msg = engine_event_to_server_message(event);
        match msg.event {
            Some(kezen_proto::server_message::Event::ThinkingDelta(t)) => assert_eq!(t.text, "thinking"),
            _ => panic!("Expected ThinkingDelta variant"),
        }

        // CostUpdate
        let event = EngineEvent::CostUpdate(crate::api::types::Usage {
            input_tokens: 10,
            output_tokens: 20,
            cache_creation_input_tokens: 5,
            cache_read_input_tokens: 15,
        });
        let msg = engine_event_to_server_message(event);
        match msg.event {
            Some(kezen_proto::server_message::Event::CostUpdate(c)) => {
                let usage = c.usage.unwrap();
                assert_eq!(usage.input_tokens, 10);
                assert_eq!(usage.output_tokens, 20);
                assert_eq!(usage.cache_creation_input_tokens, 5);
                assert_eq!(usage.cache_read_input_tokens, 15);
            }
            _ => panic!("Expected CostUpdate variant"),
        }

        // ToolUseStart
        let event = EngineEvent::ToolUseStart {
            id: "id-123".to_string(),
            name: "test_tool".to_string(),
            input: serde_json::json!({"arg": 1}),
        };
        let msg = engine_event_to_server_message(event);
        match msg.event {
            Some(kezen_proto::server_message::Event::ToolUseStart(t)) => {
                assert_eq!(t.tool_use_id, "id-123");
                assert_eq!(t.name, "test_tool");
                assert_eq!(t.input_json, "{\"arg\":1}");
            }
            _ => panic!("Expected ToolUseStart variant"),
        }

        // ToolResult
        let event = EngineEvent::ToolResult {
            id: "id-123".to_string(),
            output: "success".to_string(),
            is_error: false,
        };
        let msg = engine_event_to_server_message(event);
        match msg.event {
            Some(kezen_proto::server_message::Event::ToolResult(t)) => {
                assert_eq!(t.tool_use_id, "id-123");
                assert_eq!(t.output, "success");
                assert_eq!(t.is_error, false);
            }
            _ => panic!("Expected ToolResult variant"),
        }

        // PermissionRequest
        let event = EngineEvent::PermissionRequest {
            id: "req-1".to_string(),
            tool: "dangerous_tool".to_string(),
            description: "Will do something bad".to_string(),
            risk_level: crate::permissions::RiskLevel::High,
            suggestion: Some("allow_this".to_string()),
        };
        let msg = engine_event_to_server_message(event);
        match msg.event {
            Some(kezen_proto::server_message::Event::PermissionRequest(p)) => {
                assert_eq!(p.request_id, "req-1");
                assert_eq!(p.tool, "dangerous_tool");
                assert_eq!(p.description, "Will do something bad");
                assert_eq!(p.risk_level, kezen_proto::RiskLevel::High as i32);
                assert_eq!(p.suggestions, vec!["allow_this".to_string()]);
            }
            _ => panic!("Expected PermissionRequest variant"),
        }

        // SlashCommandResult
        let event = EngineEvent::SlashCommandResult {
            command: "/status".to_string(),
            output: "OK".to_string(),
        };
        let msg = engine_event_to_server_message(event);
        match msg.event {
            Some(kezen_proto::server_message::Event::SlashCommandResult(s)) => {
                assert_eq!(s.command, "/status");
                assert_eq!(s.output, "OK");
            }
            _ => panic!("Expected SlashCommandResult variant"),
        }

        // CompactProgress
        let event = EngineEvent::CompactProgress { message: "compressing...".to_string() };
        let msg = engine_event_to_server_message(event);
        match msg.event {
            Some(kezen_proto::server_message::Event::CompactProgress(c)) => assert_eq!(c.message, "compressing..."),
            _ => panic!("Expected CompactProgress variant"),
        }

        // SkillLoaded
        let event = EngineEvent::SkillLoaded { name: "my_skill".to_string() };
        let msg = engine_event_to_server_message(event);
        match msg.event {
            Some(kezen_proto::server_message::Event::SkillLoaded(s)) => assert_eq!(s.name, "my_skill"),
            _ => panic!("Expected SkillLoaded variant"),
        }

        // Done
        let event = EngineEvent::Done;
        let msg = engine_event_to_server_message(event);
        match msg.event {
            Some(kezen_proto::server_message::Event::Done(_)) => {}
            _ => panic!("Expected Done variant"),
        }

        // Error
        let event = EngineEvent::Error { message: "system failure".to_string() };
        let msg = engine_event_to_server_message(event);
        match msg.event {
            Some(kezen_proto::server_message::Event::Error(e)) => assert_eq!(e.message, "system failure"),
            _ => panic!("Expected Error variant"),
        }

        // Warning
        let event = EngineEvent::Warning("rate limit".to_string());
        let msg = engine_event_to_server_message(event);
        match msg.event {
            Some(kezen_proto::server_message::Event::Warning(w)) => assert_eq!(w.message, "rate limit"),
            _ => panic!("Expected Warning variant"),
        }

        // SessionRestored
        let event = EngineEvent::SessionRestored {
            messages: vec![
                crate::api::types::Message {
                    role: crate::api::types::Role::User,
                    content: vec![crate::api::types::ContentBlock::Text { text: "hello".into() }],
                },
                crate::api::types::Message {
                    role: crate::api::types::Role::Assistant,
                    content: vec![crate::api::types::ContentBlock::Text { text: "world".into() }],
                },
            ],
        };
        let msg = engine_event_to_server_message(event);
        match msg.event {
            Some(kezen_proto::server_message::Event::SessionRestored(sr)) => {
                assert!(!sr.messages_json.is_empty());
                // Verify it's valid JSON containing our messages
                let parsed: Vec<serde_json::Value> = serde_json::from_str(&sr.messages_json).unwrap();
                assert_eq!(parsed.len(), 2);
            }
            _ => panic!("Expected SessionRestored variant"),
        }
    }
}

