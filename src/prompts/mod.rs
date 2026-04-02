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
        DOING_TASKS.to_string(),
        TONE_AND_STYLE_BASE.to_string(),
        OUTPUT_EFFICIENCY.to_string(),
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
