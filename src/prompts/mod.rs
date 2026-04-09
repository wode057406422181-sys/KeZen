use std::env;

use crate::constants::prompts::*;
use crate::skills::registry::SkillRegistry;

fn get_simple_intro_section() -> String {
    format!(
        "You are an interactive agent that helps users with software engineering tasks. Use the instructions below and the tools available to you to assist the user.\n\n{}\nIMPORTANT: You must NEVER generate or guess URLs for the user unless you are confident that the URLs are for helping the user with programming. You may use URLs provided by the user in their messages or local files.",
        CYBER_RISK_INSTRUCTION
    )
}

async fn compute_env_info(work_dir: &std::path::Path, model: Option<&str>) -> String {
    let os_type = env::consts::OS;
    let os_version = env::consts::FAMILY;
    let shell = env::var("SHELL").unwrap_or_else(|_| "unknown".to_string());
    let cwd = work_dir.display().to_string();

    let mut is_git = false;
    if let Ok(output) = tokio::process::Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(work_dir)
        .output()
        .await
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

pub async fn build_static_system_prompt(
    work_dir: &std::path::Path,
    model: Option<&str>,
    skill_registry: Option<&SkillRegistry>,
) -> String {
    let elements = [
        get_simple_intro_section(),
        SYSTEM_RULES.to_string(),
        DOING_TASKS.to_string(),
        ACTIONS.to_string(),
        USING_TOOLS.to_string(),
        TONE_AND_STYLE.to_string(),
        OUTPUT_EFFICIENCY.to_string(),
        SESSION_GUIDANCE.to_string(),
        SYSTEM_PROMPT_DYNAMIC_BOUNDARY.to_string(),
        compute_env_info(work_dir, model).await,
    ];

    let mut prompt = elements.join("\n\n");

    prompt.push_str(&format!(
        "\n\n# Current Working Directory\n{}\n",
        work_dir.display()
    ));

    // Memory files are treated as static for cache hit rate (they rarely change block sizes within a single conversation)
    let memory_files = crate::context::memory::load_memory_files(work_dir).await;
    if let Some(memory_prompt) = crate::context::memory::format_memory_prompt(&memory_files) {
        prompt.push_str("\n\n");
        prompt.push_str(&memory_prompt);
    }

    if let Some(registry) = skill_registry {
        if !registry.all().is_empty() {
            let listing =
                registry.format_listing(crate::constants::defaults::DEFAULT_SKILL_BUDGET_CHARS);

            let first_skill = registry.all().keys().next().unwrap();

            prompt.push_str("\n\n<skills>\n");
            prompt.push_str("# Available Skills\n\n");
            prompt.push_str("The following skills are available via the Skill tool:\n\n");
            prompt.push_str(&listing);
            prompt.push_str("\n\n");
            prompt.push_str("When a user references a \"slash command\" like \"/");
            prompt.push_str(first_skill);
            prompt.push_str("\", they are referring to a skill listed above.\n\n");
            prompt.push_str("**BLOCKING REQUIREMENT**: When a skill matches the user's request, ");
            prompt
                .push_str("invoke it via the Skill tool BEFORE generating any other response.\n\n");
            prompt.push_str("Invocation examples:\n");
            prompt.push_str(&format!(
                "  - `skill: \"{}\"` — invoke the {} skill\n",
                first_skill, first_skill
            ));
            prompt.push_str(&format!(
                "  - `skill: \"{}\", args: \"<arguments>\"` — with arguments\n",
                first_skill
            ));
            prompt.push_str("\n</skills>");
        }
    }

    prompt
}

pub fn build_dynamic_context(git_ctx: Option<&crate::context::git::GitContext>) -> String {
    let mut context = String::new();
    
    // ISO 8601 current time
    let now = chrono::Local::now();
    context.push_str(&format!("Current Time: {}\n", now.format("%Y-%m-%dT%H:%M:%S%z")));

    if let Some(git) = git_ctx {
        context.push_str("\n# Git Context\n");
        context.push_str(&format!("Branch: {}\n", git.branch));
        context.push_str(&format!("Default Branch: {}\n", git.default_branch));
        context.push_str(&format!("Recent Commits:\n{}\n", git.recent_commits));
        context.push_str(&format!("Status:\n{}\n", git.status));
    }

    context
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── build_system_prompt section presence ──────────────────────────────────

    #[tokio::test]
    async fn prompt_contains_intro_sentinel() {
        let prompt = build_static_system_prompt(std::path::Path::new("."), None, None).await;
        assert!(
            prompt.contains("interactive agent"),
            "Prompt must contain intro section"
        );
    }

    #[tokio::test]
    async fn prompt_contains_dynamic_boundary_marker() {
        let prompt = build_static_system_prompt(std::path::Path::new("."), None, None).await;
        assert!(
            prompt.contains(SYSTEM_PROMPT_DYNAMIC_BOUNDARY),
            "Prompt must contain the dynamic boundary marker used for runtime injection"
        );
    }

    #[tokio::test]
    async fn prompt_contains_tone_section() {
        let prompt = build_static_system_prompt(std::path::Path::new("."), None, None).await;
        assert!(
            prompt.contains("# Tone and style"),
            "Prompt must include tone and style section"
        );
    }

    #[tokio::test]
    async fn prompt_contains_output_efficiency_section() {
        let prompt = build_static_system_prompt(std::path::Path::new("."), None, None).await;
        assert!(
            prompt.contains("# Output efficiency"),
            "Prompt must include output efficiency section"
        );
    }

    #[tokio::test]
    async fn prompt_contains_environment_header() {
        let prompt = build_static_system_prompt(std::path::Path::new("."), None, None).await;
        assert!(
            prompt.contains("# Environment"),
            "Prompt must include the Environment section"
        );
    }

    #[tokio::test]
    async fn prompt_contains_platform_info() {
        let prompt = build_static_system_prompt(std::path::Path::new("."), None, None).await;
        let expected_os = std::env::consts::OS;
        assert!(
            prompt.contains(expected_os),
            "Prompt must contain the current platform OS string"
        );
    }

    // ── Model injection ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn prompt_injects_model_name_when_provided() {
        let prompt = build_static_system_prompt(std::path::Path::new("."), Some("claude-opus-4-5"), None).await;
        assert!(
            prompt.contains("claude-opus-4-5"),
            "Model name should appear in the Environment section"
        );
    }

    #[tokio::test]
    async fn prompt_without_model_has_no_model_line() {
        let prompt = build_static_system_prompt(std::path::Path::new("."), None, None).await;
        assert!(
            !prompt.contains("You are powered by the model"),
            "Without a model arg, model description should not appear"
        );
    }

    // ── Section ordering ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn environment_section_comes_after_dynamic_boundary() {
        let prompt = build_static_system_prompt(std::path::Path::new("."), None, None).await;
        let boundary_pos = prompt.find(SYSTEM_PROMPT_DYNAMIC_BOUNDARY).unwrap();
        let env_pos = prompt.find("# Environment").unwrap();
        assert!(
            env_pos > boundary_pos,
            "# Environment must appear after the dynamic boundary marker"
        );
    }

    // ── compute_env_info (pure parts) ─────────────────────────────────────────

    #[tokio::test]
    async fn env_info_with_model_contains_model_name() {
        let info = compute_env_info(std::path::Path::new("."), Some("test-model-xyz")).await;
        assert!(info.contains("test-model-xyz"));
    }

    #[tokio::test]
    async fn env_info_without_model_omits_model_line() {
        let info = compute_env_info(std::path::Path::new("."), None).await;
        assert!(!info.contains("You are powered by the model"));
    }

    #[tokio::test]
    async fn env_info_always_contains_platform() {
        let info = compute_env_info(std::path::Path::new("."), None).await;
        assert!(info.contains(std::env::consts::OS));
    }
}
