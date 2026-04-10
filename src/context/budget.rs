/// Approximate tokens from character count.
/// Uses ~3.5 chars/token heuristic (reasonable for English + code mixed content).
fn estimate_tokens(text: &str) -> u64 {
    (text.chars().count() as f64 / 3.5).ceil() as u64
}

pub struct ContextBudgetManager {
    max_tool_result_tokens: u64,
}

impl ContextBudgetManager {
    pub fn new(max_tool_result_tokens: u64) -> Self {
        Self {
            max_tool_result_tokens,
        }
    }

    /// Truncate tool result if it exceeds the budget
    pub fn enforce_tool_budget(&self, result: &str) -> String {
        let estimated_tokens = estimate_tokens(result);
        if estimated_tokens > self.max_tool_result_tokens {
            let total_chars = result.chars().count();
            let ratio = self.max_tool_result_tokens as f64 / estimated_tokens as f64;
            let chars_to_keep = (total_chars as f64 * ratio) as usize;
            let safe_chars = chars_to_keep.saturating_sub(150);

            let truncated_str: String = result.chars().take(safe_chars).collect();

            format!(
                "{}\n\n[Output truncated from {} to ~{} tokens. Use targeted queries for specific content.]",
                truncated_str, estimated_tokens, self.max_tool_result_tokens
            )
        } else {
            result.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enforce_budget_truncation() {
        let mgr = ContextBudgetManager::new(10); // artificially low limit

        // at 3.5 chars/token, 100 chars ≈ 29 tokens > 10
        let long_str = "a".repeat(100);
        let result = mgr.enforce_tool_budget(&long_str);

        assert!(result.contains("[Output truncated"));
        assert!(result.len() < long_str.len() + 100);
    }

    #[test]
    fn test_enforce_budget_no_truncation() {
        let mgr = ContextBudgetManager::new(1000);

        let s = "Hello, world!".to_string();
        let result = mgr.enforce_tool_budget(&s);
        assert_eq!(result, s);
    }

    #[test]
    fn test_estimate_tokens_basic() {
        // 35 chars / 3.5 = 10 tokens
        assert_eq!(estimate_tokens(&"a".repeat(35)), 10);
    }
}
