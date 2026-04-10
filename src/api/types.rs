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

    /// Tool use block
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
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
}

/// Unified stream events from LLM providers.
///
/// These events are consumed by the Engine to build EngineEvents
/// for the frontend.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// Incremental text content
    TextDelta {
        text: String,
    },
    /// Incremental thinking content (Anthropic extended thinking)
    ThinkingDelta {
        text: String,
    },
    /// A content block started (index + type for routing)
    ContentBlockStart {
        index: usize,
        block_type: String,
    },
    /// A content block ended
    ContentBlockStop {
        index: usize,
    },
    /// Message started (role + initial usage from Anthropic)
    MessageStart {
        role: Role,
        usage: Option<Usage>,
    },
    /// Message delta (stop reason for tool loop + final usage)
    MessageDelta {
        stop_reason: Option<String>,
        usage: Option<Usage>,
    },
    /// Message stream fully ended
    MessageStop,

    /// Tool use streaming events
    ToolUseStart {
        id: String,
        name: String,
    },
    ToolUseInputDelta {
        text: String,
    },
    ToolUseInputDone,
}
