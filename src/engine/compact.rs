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

#[cfg(test)]
mod tests {
    use super::*;

    // ── context_window_for_model ─────────────────────────────────────

    #[test]
    fn context_window_claude_models() {
        assert_eq!(context_window_for_model("claude-3-5-sonnet-20241022"), 200_000);
        assert_eq!(context_window_for_model("claude-3-opus-20240229"), 200_000);
        assert_eq!(context_window_for_model("claude-3-haiku-20240307"), 200_000);
    }

    #[test]
    fn context_window_gpt4o() {
        assert_eq!(context_window_for_model("gpt-4o-2024-05-13"), 128_000);
    }

    #[test]
    fn context_window_gemini_pro() {
        assert_eq!(context_window_for_model("gemini-2.0-pro"), 1_000_000);
    }

    #[test]
    fn context_window_unknown_model_defaults() {
        assert_eq!(context_window_for_model("llama-3.1-70b"), 128_000);
    }

    // ── should_auto_compact ──────────────────────────────────────────

    #[test]
    fn auto_compact_triggers_above_threshold() {
        // 80% of 200k = 160k, so 160001 should trigger
        assert!(should_auto_compact(160_001, "claude-3-5-sonnet-20241022", None));
    }

    #[test]
    fn auto_compact_does_not_trigger_below_threshold() {
        assert!(!should_auto_compact(100_000, "claude-3-5-sonnet-20241022", None));
    }

    #[test]
    fn auto_compact_respects_configured_window() {
        // 80% of 50k = 40k
        assert!(should_auto_compact(40_001, "anything", Some(50_000)));
        assert!(!should_auto_compact(39_999, "anything", Some(50_000)));
    }

    #[test]
    fn auto_compact_at_exact_threshold_does_not_trigger() {
        // 80% of 200k = 160000 exactly — should NOT trigger (> not >=)
        assert!(!should_auto_compact(160_000, "claude-3-5-sonnet-20241022", None));
    }

    // ── compact_prompt ───────────────────────────────────────────────

    #[test]
    fn compact_prompt_includes_all_parts() {
        let prompt = compact_prompt();
        assert!(prompt.contains("CRITICAL: Respond with TEXT ONLY"));
        assert!(prompt.contains("Your task is to create a detailed summary"));
        assert!(prompt.contains("REMINDER: Do NOT call any tools"));
    }

    // ── validate_and_extract ─────────────────────────────────────────

    #[test]
    fn validate_empty_response_no_errors() {
        let result = validate_and_extract("", &[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty response"));
    }

    #[test]
    fn validate_empty_response_with_stream_errors() {
        let errors = vec!["timeout".to_string()];
        let result = validate_and_extract("   ", &errors);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("timeout"));
    }

    #[test]
    fn validate_proper_summary_tags() {
        let raw = "<analysis>thinking...</analysis><summary>The user asked about X.</summary>";
        let (summary, warnings) = validate_and_extract(raw, &[]).unwrap();
        assert_eq!(summary, "The user asked about X.");
        assert!(warnings.is_empty());
    }

    #[test]
    fn validate_missing_summary_tags_warns() {
        let raw = "Here is a plain text summary without tags.";
        let (summary, warnings) = validate_and_extract(raw, &[]).unwrap();
        assert_eq!(summary, raw);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("missing <summary> tags"));
    }

    #[test]
    fn validate_summary_only_open_tag() {
        let raw = "<summary>content without closing tag";
        let (summary, warnings) = validate_and_extract(raw, &[]).unwrap();
        assert_eq!(summary, "content without closing tag");
        // Has <summary> but not </summary>
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn validate_extracts_empty_after_parse_fails() {
        let raw = "<summary>   </summary>";
        let result = validate_and_extract(raw, &[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty after parsing"));
    }

    // ── extract_summary (private, tested via validate_and_extract) ───

    #[test]
    fn extract_strips_analysis_block() {
        let raw = "<analysis>scratchpad</analysis><summary>real content</summary>";
        let (summary, _) = validate_and_extract(raw, &[]).unwrap();
        assert_eq!(summary, "real content");
        assert!(!summary.contains("scratchpad"));
    }

    #[test]
    fn extract_unclosed_analysis_preserves_raw() {
        let raw = "<analysis>unclosed\n<summary>content here</summary>";
        let (summary, _) = validate_and_extract(raw, &[]).unwrap();
        assert_eq!(summary, "content here");
    }

    #[test]
    fn extract_whitespace_trimmed() {
        let raw = "<summary>  \n  trimmed content  \n  </summary>";
        let (summary, _) = validate_and_extract(raw, &[]).unwrap();
        assert_eq!(summary, "trimmed content");
    }

    #[test]
    fn extract_no_tags_fallback() {
        let raw = "  plain text fallback  ";
        let (summary, _) = validate_and_extract(raw, &[]).unwrap();
        assert_eq!(summary, "plain text fallback");
    }
}

