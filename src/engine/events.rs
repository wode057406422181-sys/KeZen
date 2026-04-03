use crate::api::types::Usage;

/// Events sent from Engine to Frontend
#[derive(Debug, Clone)]
pub enum EngineEvent {
    /// Incremental AI text response
    TextDelta { text: String },
    /// Incremental thinking process (Anthropic extended thinking)
    ThinkingDelta { text: String },
    /// Token usage update for the current turn
    CostUpdate(Usage),
    /// Error message from the engine
    Error { message: String },
    /// Current turn is complete
    Done,
    /// Tool execution started
    ToolUseStart {
        #[allow(dead_code)] // reserved for permission gating
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// Tool execution result
    ToolResult {
        #[allow(dead_code)] // reserved for permission gating
        id: String,
        output: String,
        is_error: bool,
    },
    /// Request user permission for a potentially unsafe tool invocation
    PermissionRequest {
        id: String,
        tool: String,
        description: String,
    },
    /// Provide current session snapshot to frontend
    SessionSnapshotUpdate {
        snapshot: crate::session::SessionSnapshot,
    },
}

/// Actions sent from Frontend to Engine
#[derive(Debug, Clone)]
pub enum UserAction {
    /// User sends a chat message
    SendMessage { content: String },
    /// User cancels the current streaming response
    Cancel,
    /// User responds to a permission request
    PermissionResponse {
        id: String,
        allowed: bool,
        always_allow: bool,
    },
    /// Restore a previous session state
    RestoreSession {
        snapshot: crate::session::SessionSnapshot,
    },
}
