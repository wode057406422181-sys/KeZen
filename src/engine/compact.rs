use crate::constants::prompts::{
    COMPACT_NO_TOOLS_PREAMBLE, COMPACT_NO_TOOLS_TRAILER, COMPACT_PROMPT,
};

pub use crate::constants::api::COMPACT_MAX_OUTPUT_TOKENS;

/// Helper to decide if auto-compaction should trigger.
///
/// Uses the real `input_tokens` from the last API response (the actual context
/// window usage as measured by the API's tokenizer) instead of a local estimate.
pub fn should_auto_compact(last_input_tokens: u64, context_window: u64) -> bool {
    if context_window == 0 {
        return false;
    }
    let percent = (last_input_tokens as f64 / context_window as f64) * 100.0;
    percent > 80.0
}

/// Build the full compact prompt: NO_TOOLS preamble + main prompt + NO_TOOLS trailer
pub fn compact_prompt() -> String {
    format!(
        "{}{}{}",
        COMPACT_NO_TOOLS_PREAMBLE, COMPACT_PROMPT, COMPACT_NO_TOOLS_TRAILER
    )
}

/// Validate raw LLM output and extract the summary.
///
/// Returns `Ok((summary, warnings))` on success, `Err(reason)` on failure.
/// Warnings are non-fatal issues (e.g. missing tags) that the caller can forward to the user.
pub fn validate_and_extract(
    raw: &str,
    stream_errors: &[String],
) -> Result<(String, Vec<String>), String> {
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
            return without_analysis[content_start..content_start + end_idx]
                .trim()
                .to_string();
        }
        return without_analysis[content_start..].trim().to_string();
    }

    // Fallback if tags not present
    without_analysis.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── should_auto_compact ──────────────────────────────────────────

    #[test]
    fn auto_compact_triggers_above_threshold() {
        // 162,000 / 200,000 = 81%
        assert!(should_auto_compact(162_000, 200_000));
    }

    #[test]
    fn auto_compact_does_not_trigger_below_threshold() {
        // 100,000 / 200,000 = 50%
        assert!(!should_auto_compact(100_000, 200_000));
    }

    #[test]
    fn auto_compact_at_exact_threshold_does_not_trigger() {
        // 160,000 / 200,000 = 80.0%
        assert!(!should_auto_compact(160_000, 200_000));
    }

    #[test]
    fn auto_compact_zero_window_does_not_trigger() {
        assert!(!should_auto_compact(100, 0));
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
