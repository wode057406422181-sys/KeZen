use crate::api::types::{Message, Usage};

/// Maintains conversation history and cumulative token usage for a session.
pub struct Session {
    messages: Vec<Message>,
    total_input_tokens: u32,
    total_output_tokens: u32,
}

impl Session {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            total_input_tokens: 0,
            total_output_tokens: 0,
        }
    }

    /// Append a message (user or assistant) to the conversation history.
    pub fn add_message(&mut self, message: Message) {
        self.messages.push(message);
    }

    /// Get a snapshot of the current message history.
    pub fn messages(&self) -> Vec<Message> {
        self.messages.clone()
    }

    /// Accumulate token usage from a single turn.
    pub fn update_usage(&mut self, usage: &Usage) {
        self.total_input_tokens += usage.input_tokens;
        self.total_output_tokens += usage.output_tokens;
    }

    /// Get cumulative usage across all turns.
    #[allow(dead_code)]
    pub fn total_usage(&self) -> Usage {
        Usage {
            input_tokens: self.total_input_tokens,
            output_tokens: self.total_output_tokens,
        }
    }

    /// Clear all messages (for /clear command).
    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.messages.clear();
    }
}
