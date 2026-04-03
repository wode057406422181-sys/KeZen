use crate::api::types::{Message, Usage};
use crate::cost::{calculate_cost, CostPricing};

/// Maintains conversation history and cumulative token usage for a session.
pub struct Session {
    pub id: String,
    pub created_at: String,
    messages: Vec<Message>,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pricing: CostPricing,
    pub total_cost_usd: f64,
}

impl Session {
    pub fn new(pricing: CostPricing) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            messages: Vec::new(),
            total_input_tokens: 0,
            total_output_tokens: 0,
            pricing,
            total_cost_usd: 0.0,
        }
    }

    pub fn restore(&mut self, snap: crate::session::SessionSnapshot) {
        self.id = snap.id;
        self.created_at = snap.created_at;
        self.messages = snap.messages;
        self.total_input_tokens = snap.input_tokens;
        self.total_output_tokens = snap.output_tokens;
        self.total_cost_usd = snap.cost_usd;
    }

    pub fn snapshot(&self) -> crate::session::SessionSnapshot {
        crate::session::SessionSnapshot {
            id: self.id.clone(),
            created_at: self.created_at.clone(),
            updated_at: chrono::Utc::now().to_rfc3339(),
            messages: self.messages.clone(),
            input_tokens: self.total_input_tokens,
            output_tokens: self.total_output_tokens,
            cost_usd: self.total_cost_usd,
        }
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
        self.total_cost_usd = calculate_cost(
            self.total_input_tokens as u32,
            self.total_output_tokens as u32,
            &self.pricing,
        );
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
