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

/// Validate raw LLM output and extract the summary.
///
/// Returns `Ok((summary, warnings))` on success, `Err(reason)` on failure.
/// Warnings are non-fatal issues (e.g. missing tags) that the caller can forward to the user.
pub fn validate_and_extract(raw: &str, stream_errors: &[String]) -> Result<(String, Vec<String>), String> {
    let mut warnings = Vec::new();

    // Guard: empty response
    if raw.trim().is_empty() {
        let reason = if stream_errors.is_empty() {
            "LLM returned empty response".to_string()
        } else {
            format!("Stream errors: {}", stream_errors.join("; "))
        };
        return Err(reason);
    }

    // Check for proper <summary> tags
    if !raw.contains("<summary>") || !raw.contains("</summary>") {
        warnings.push("LLM response missing <summary> tags, using raw output.".to_string());
    }

    let summary = extract_summary(raw);

    // Final guard: extracted summary must not be empty
    if summary.trim().is_empty() {
        return Err("Extracted summary is empty after parsing".to_string());
    }

    Ok((summary, warnings))
}

fn extract_summary(raw: &str) -> String {
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
