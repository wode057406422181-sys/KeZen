use crate::constants::prompts::{COMPACT_NO_TOOLS_PREAMBLE, COMPACT_PROMPT, COMPACT_NO_TOOLS_TRAILER};

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
pub fn should_auto_compact(total_input_tokens: u64, model: &str, configured_window: Option<u64>) -> bool {
    let window = configured_window.unwrap_or_else(|| context_window_for_model(model));
    let threshold = (window as f64 * 0.80) as u64; // 80% of context window
    total_input_tokens > threshold
}

/// Build the full compact prompt: NO_TOOLS preamble + main prompt + NO_TOOLS trailer
pub fn compact_prompt() -> String {
    format!("{}{}{}", COMPACT_NO_TOOLS_PREAMBLE, COMPACT_PROMPT, COMPACT_NO_TOOLS_TRAILER)
}

pub fn extract_summary(raw: &str) -> String {
    // Strip <analysis> block first (drafting scratchpad, not needed in output)
    let without_analysis = if let Some(start) = raw.find("<analysis>") {
        if let Some(end) = raw.find("</analysis>") {
            format!("{}{}", &raw[..start], &raw[end + "</analysis>".len()..])
        } else {
            raw.to_string()
        }
    } else {
        raw.to_string()
    };

    // Extract <summary> content
    let start_tag = "<summary>";
    let end_tag = "</summary>";

    if let Some(start_idx) = without_analysis.find(start_tag) {
        let content_start = start_idx + start_tag.len();
        if let Some(end_idx) = without_analysis[content_start..].find(end_tag) {
            return without_analysis[content_start..content_start + end_idx].trim().to_string();
        }
        return without_analysis[content_start..].trim().to_string();
    }

    // Fallback if tags not present
    without_analysis.trim().to_string()
}
