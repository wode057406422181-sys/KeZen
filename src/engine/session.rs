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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::{ContentBlock, Role};

    fn zero_pricing() -> CostPricing {
        CostPricing { input_cost_per_mtoken: 0.0, output_cost_per_mtoken: 0.0 }
    }

    fn sonnet_pricing() -> CostPricing {
        CostPricing { input_cost_per_mtoken: 3.0, output_cost_per_mtoken: 15.0 }
    }

    #[test]
    fn test_new_session_is_empty() {
        let s = Session::new(zero_pricing());
        assert!(s.messages().is_empty());
        assert_eq!(s.total_input_tokens, 0);
        assert_eq!(s.total_output_tokens, 0);
        assert_eq!(s.total_cost_usd, 0.0);
    }

    #[test]
    fn test_add_and_read_messages() {
        let mut s = Session::new(zero_pricing());
        s.add_message(Message {
            role: Role::User,
            content: vec![ContentBlock::Text { text: "hello".into() }],
        });
        s.add_message(Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text { text: "hi!".into() }],
        });
        assert_eq!(s.messages().len(), 2);
        assert_eq!(s.messages()[0].role, Role::User);
        assert_eq!(s.messages()[1].role, Role::Assistant);
    }

    #[test]
    fn test_update_usage_accumulates() {
        let mut s = Session::new(sonnet_pricing());
        s.update_usage(&Usage { input_tokens: 100, output_tokens: 50 });
        assert_eq!(s.total_input_tokens, 100);
        assert_eq!(s.total_output_tokens, 50);

        s.update_usage(&Usage { input_tokens: 200, output_tokens: 100 });
        assert_eq!(s.total_input_tokens, 300);
        assert_eq!(s.total_output_tokens, 150);
        assert!(s.total_cost_usd > 0.0);
    }

    #[test]
    fn test_clear_empties_messages() {
        let mut s = Session::new(zero_pricing());
        s.add_message(Message {
            role: Role::User,
            content: vec![ContentBlock::Text { text: "hello".into() }],
        });
        assert_eq!(s.messages().len(), 1);
        s.clear();
        assert!(s.messages().is_empty());
    }

    #[test]
    fn test_total_usage() {
        let mut s = Session::new(zero_pricing());
        s.update_usage(&Usage { input_tokens: 500, output_tokens: 200 });
        let u = s.total_usage();
        assert_eq!(u.input_tokens, 500);
        assert_eq!(u.output_tokens, 200);
    }

    #[test]
    fn test_snapshot_roundtrip() {
        let mut s = Session::new(sonnet_pricing());
        s.add_message(Message {
            role: Role::User,
            content: vec![ContentBlock::Text { text: "test msg".into() }],
        });
        s.update_usage(&Usage { input_tokens: 1000, output_tokens: 500 });

        let snap = s.snapshot();
        assert_eq!(snap.id, s.id);
        assert_eq!(snap.input_tokens, 1000);
        assert_eq!(snap.output_tokens, 500);
        assert_eq!(snap.messages.len(), 1);

        // Restore into a new session
        let mut s2 = Session::new(sonnet_pricing());
        s2.restore(snap);
        assert_eq!(s2.id, s.id);
        assert_eq!(s2.total_input_tokens, 1000);
        assert_eq!(s2.messages().len(), 1);
    }
}
