use std::env;
use std::process::Command;

use crate::constants::prompts::*;

fn get_simple_intro_section() -> String {
    format!(
        "You are an interactive agent that helps users with software engineering tasks. Use the instructions below and the tools available to you to assist the user.\n\n{}\nIMPORTANT: You must NEVER generate or guess URLs for the user unless you are confident that the URLs are for helping the user with programming. You may use URLs provided by the user in their messages or local files.",
        CYBER_RISK_INSTRUCTION
    )
}

fn compute_env_info(model: Option<&str>) -> String {
    let os_type = env::consts::OS;
    let os_version = env::consts::FAMILY;
    let shell = env::var("SHELL").unwrap_or_else(|_| "unknown".to_string());
    let cwd = env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    let mut is_git = false;
    if let Ok(output) = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        && output.status.success()
    {
        let res = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if res == "true" {
            is_git = true;
        }
    }

    let mut model_description = String::new();
    if let Some(m) = model {
        model_description = format!(" - You are powered by the model {}.", m);
    }

    let env_items = vec![
        format!(" - Primary working directory: {}", cwd),
        format!(" - Is a git repository: {}", is_git),
        format!(" - Platform: {}", os_type),
        format!(" - Shell: {}", shell),
        format!(" - OS Version: {}", os_version),
        model_description,
    ];

    let filtered_items: Vec<String> = env_items.into_iter().filter(|x| !x.is_empty()).collect();

    format!(
        "# Environment\nYou have been invoked in the following environment: \n{}",
        filtered_items.join("\n")
    )
}

pub fn build_system_prompt(model: Option<&str>) -> String {
    // Only fetch elements that are active for Phase 1.
    // To enable Phase 2 sections (like ACTIONS_PHASE2, USING_TOOLS_PHASE2), just add them here.
    let elements = [
        get_simple_intro_section(),
        SYSTEM_BASE.to_string(),
        SYSTEM_TOOLS_PHASE2.to_string(),
        DOING_TASKS.to_string(),
        ACTIONS_PHASE2.to_string(),
        USING_TOOLS_PHASE2.to_string(),
        TONE_AND_STYLE_BASE.to_string(),
        TONE_AND_STYLE_TOOLS_PHASE2.to_string(),
        OUTPUT_EFFICIENCY.to_string(),
        SESSION_GUIDANCE_PHASE2.to_string(),
        SYSTEM_PROMPT_DYNAMIC_BOUNDARY.to_string(),
        compute_env_info(model),
    ];

    let mut prompt = elements.join("\n\n");

    // Try reading .infini.md (Similar to CLAUDE.md)
    if let Ok(content) = std::fs::read_to_string(".infini.md") {
        prompt.push_str("\n\n# Project Memory\n");
        prompt.push_str(&content);
    }

    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── build_system_prompt section presence ──────────────────────────────────

    #[test]
    fn prompt_contains_intro_sentinel() {
        let prompt = build_system_prompt(None);
        assert!(
            prompt.contains("interactive agent"),
            "Prompt must contain intro section"
        );
    }

    #[test]
    fn prompt_contains_dynamic_boundary_marker() {
        let prompt = build_system_prompt(None);
        assert!(
            prompt.contains(SYSTEM_PROMPT_DYNAMIC_BOUNDARY),
            "Prompt must contain the dynamic boundary marker used for runtime injection"
        );
    }

    #[test]
    fn prompt_contains_tone_section() {
        let prompt = build_system_prompt(None);
        // TONE_AND_STYLE_BASE starts with "# Tone and style"
        assert!(
            prompt.contains("# Tone and style"),
            "Prompt must include tone and style section"
        );
    }

    #[test]
    fn prompt_contains_output_efficiency_section() {
        let prompt = build_system_prompt(None);
        assert!(
            prompt.contains("# Output efficiency"),
            "Prompt must include output efficiency section"
        );
    }

    #[test]
    fn prompt_contains_environment_header() {
        let prompt = build_system_prompt(None);
        assert!(
            prompt.contains("# Environment"),
            "Prompt must include the Environment section"
        );
    }

    #[test]
    fn prompt_contains_platform_info() {
        let prompt = build_system_prompt(None);
        // Platform is always injected; value matches std::env::consts::OS
        let expected_os = std::env::consts::OS;
        assert!(
            prompt.contains(expected_os),
            "Prompt must contain the current platform OS string"
        );
    }

    // ── Model injection ───────────────────────────────────────────────────────

    #[test]
    fn prompt_injects_model_name_when_provided() {
        let prompt = build_system_prompt(Some("claude-opus-4-5"));
        assert!(
            prompt.contains("claude-opus-4-5"),
            "Model name should appear in the Environment section"
        );
    }

    #[test]
    fn prompt_without_model_has_no_model_line() {
        let prompt = build_system_prompt(None);
        assert!(
            !prompt.contains("You are powered by the model"),
            "Without a model arg, model description should not appear"
        );
    }

    // ── Section ordering ──────────────────────────────────────────────────────

    #[test]
    fn environment_section_comes_after_dynamic_boundary() {
        let prompt = build_system_prompt(None);
        let boundary_pos = prompt.find(SYSTEM_PROMPT_DYNAMIC_BOUNDARY).unwrap();
        let env_pos = prompt.find("# Environment").unwrap();
        assert!(
            env_pos > boundary_pos,
            "# Environment must appear after the dynamic boundary marker"
        );
    }

    // ── compute_env_info (pure parts) ─────────────────────────────────────────

    #[test]
    fn env_info_with_model_contains_model_name() {
        let info = compute_env_info(Some("test-model-xyz"));
        assert!(info.contains("test-model-xyz"));
    }

    #[test]
    fn env_info_without_model_omits_model_line() {
        let info = compute_env_info(None);
        assert!(!info.contains("You are powered by the model"));
    }

    #[test]
    fn env_info_always_contains_platform() {
        let info = compute_env_info(None);
        assert!(info.contains(std::env::consts::OS));
    }
}
