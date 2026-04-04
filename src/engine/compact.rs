/// Returns the context window size for a given model.
pub fn context_window_for_model(model: &str) -> u64 {
    if model.contains("opus") || model.contains("sonnet") || model.contains("haiku") {
        200_000
    } else if model.contains("gpt-4o") {
        128_000
    } else if model.contains("gemini") && model.contains("pro") {
        1_000_000
    } else {
        128_000 // Default safe value
    }
}

/// Helper to decide if auto-compaction should trigger
pub fn should_auto_compact(total_input_tokens: u64, model: &str) -> bool {
    let window = context_window_for_model(model);
    let threshold = (window as f64 * 0.80) as u64; // 80% of context window
    total_input_tokens > threshold
}

pub fn compact_prompt() -> String {
    r#"CRITICAL: Respond with TEXT ONLY. Do NOT call any tools.

Your task is to create a detailed summary of the conversation so far.

<analysis>
Write your thought process and scratchpad here.
</analysis>
<summary>
Write the final structured summary here, including:
1. User's original request and true intent
2. Key technical concepts discussed
3. Files modified and code changed
4. Errors encountered and how they were fixed
5. Pending tasks remaining
6. Current state of the work
</summary>"#.to_string()
}

pub fn extract_summary(raw: &str) -> String {
    let start_tag = "<summary>";
    let end_tag = "</summary>";
    
    if let Some(start_idx) = raw.find(start_tag) {
        let content_start = start_idx + start_tag.len();
        if let Some(end_idx) = raw[content_start..].find(end_tag) {
            return raw[content_start..content_start + end_idx].trim().to_string();
        }
        return raw[content_start..].trim().to_string();
    }
    
    // Fallback if tags not present
    raw.trim().to_string()
}
