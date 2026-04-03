use crate::api::types::{Message, Usage};

/// Maintains conversation history and cumulative token usage for a session.
pub struct Session {
    messages: Vec<Message>,
    total_input_tokens: u64,
    total_output_tokens: u64,
}

impl Session {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            total_input_tokens: 0,
            total_output_tokens: 0,
        }
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    /// Append a message (user or assistant) to the conversation history.
    pub fn add_message(&mut self, message: Message) {
        self.messages.push(message);
    }

    /// Borrow the current message history (zero-copy).
    ///
    /// Returns a slice reference instead of cloning, avoiding O(n) deep copies
    /// on every turn. Rust's field-level borrow splitting allows the engine to
    /// hold this reference while independently accessing other fields.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Accumulate token usage from a single turn.
    pub fn update_usage(&mut self, usage: &Usage) {
        self.total_input_tokens = self.total_input_tokens.saturating_add(usage.input_tokens);
        self.total_output_tokens = self.total_output_tokens.saturating_add(usage.output_tokens);
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
