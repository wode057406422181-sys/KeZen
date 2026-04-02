use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },

    #[serde(rename = "thinking")]
    Thinking { thinking: String },

    // Phase 2: tool use
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },

    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
}

#[derive(Debug, Clone, Default, Copy)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// Unified stream events from LLM providers.
///
/// These events are consumed by the Engine to build EngineEvents
/// for the frontend. Some fields are reserved for Phase 2 (tool use).
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// Incremental text content
    TextDelta { text: String },
    /// Incremental thinking content (Anthropic extended thinking)
    ThinkingDelta { text: String },
    /// A content block started (index + type for Phase 2 routing)
    ContentBlockStart {
        #[allow(dead_code)]
        index: usize,
        #[allow(dead_code)]
        block_type: String,
    },
    /// A content block ended
    ContentBlockStop {
        #[allow(dead_code)]
        index: usize,
    },
    /// Message started (role + initial usage from Anthropic)
    MessageStart {
        #[allow(dead_code)]
        role: Role,
        usage: Option<Usage>,
    },
    /// Message delta (stop reason for Phase 2 tool loop + final usage)
    MessageDelta {
        #[allow(dead_code)]
        stop_reason: Option<String>,
        usage: Option<Usage>,
    },
    /// Message stream fully ended
    MessageStop,
}
