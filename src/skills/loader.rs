use std::path::Path;
use crate::skills::types::{SkillDefinition, SkillFrontmatter, SkillSource};

/// Discover skills from all configured paths (global + project-local).
///
/// Search order:
/// 1. `~/.kezen/skills/` — user global skills
/// 2. Walk upward from `work_dir` looking for `.kezen/skills/` — project skills
///
/// Duplicates (same canonical path) are de-duplicated.
pub async fn discover_all_skills(work_dir: &Path) -> Vec<SkillDefinition> {
    let mut skills = Vec::new();
    let mut seen_canonical_paths = std::collections::HashSet::new();

    // 1. User Global Skills: ~/.kezen/skills/
    if let Some(home) = dirs::home_dir() {
        let global_skills_path = home.join(".kezen").join("skills");
        if tokio::fs::metadata(&global_skills_path).await.map(|m| m.is_dir()).unwrap_or(false) {
            tracing::debug!(path = %global_skills_path.display(), "Scanning global skills directory");
            let global_skills = load_skills_from_dir(&global_skills_path, SkillSource::UserGlobal).await;
            for skill in global_skills {
                if let Ok(canon) = tokio::fs::canonicalize(&skill.base_dir).await
                    && seen_canonical_paths.insert(canon) {
                        skills.push(skill);
                }
            }
        }
    }

    // 2. Project Local Skills: Traverse up to find .kezen/skills/
    let mut current_dir = work_dir.to_path_buf();
    loop {
        let local_skills_path = current_dir.join(".kezen").join("skills");
        if tokio::fs::metadata(&local_skills_path).await.map(|m| m.is_dir()).unwrap_or(false) {
            tracing::debug!(path = %local_skills_path.display(), "Scanning project skills directory");
            let local_skills = load_skills_from_dir(&local_skills_path, SkillSource::Project).await;
            for skill in local_skills {
                if let Ok(canon) = tokio::fs::canonicalize(&skill.base_dir).await
                    && seen_canonical_paths.insert(canon) {
                        skills.push(skill);
                }
            }
            tracing::debug!(path = %local_skills_path.display(), "Stopping traversal — found project skills directory");
            break;
        }

        // Stop at git root if one exists
        if tokio::fs::try_exists(current_dir.join(".git")).await.unwrap_or(false) {
            break;
        }

        if !current_dir.pop() {
            break;
        }
    }

    tracing::info!(count = skills.len(), "Skill discovery completed");
    skills
}

/// Load all skill definitions from a single directory.
///
/// Each subdirectory containing a `SKILL.md` file is treated as one skill.
pub async fn load_skills_from_dir(base_path: &Path, source: SkillSource) -> Vec<SkillDefinition> {
    let mut result = Vec::new();

    let mut entries = match tokio::fs::read_dir(base_path).await {
        Ok(dir) => dir,
        Err(e) => {
            tracing::warn!(path = %base_path.display(), error = %e, "Failed to read skills directory");
            return result;
        }
    };

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if let Ok(ft) = entry.file_type().await {
            if !ft.is_dir() {
                continue;
            }
        } else {
            continue;
        }

        let skill_file_path = path.join("SKILL.md");
        if !tokio::fs::metadata(&skill_file_path).await.map(|m| m.is_file()).unwrap_or(false) {
            continue;
        }

        match tokio::fs::read_to_string(&skill_file_path).await {
            Ok(content) => {
                let (frontmatter, body_length) = parse_skill_frontmatter(&content);
                let name = path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string();

                tracing::debug!(
                    skill = %name,
                    source = ?source,
                    content_bytes = body_length,
                    "Loaded skill definition"
                );

                result.push(SkillDefinition {
                    name,
                    frontmatter,
                    body_length,
                    source,
                    base_dir: path,
                });
            }
            Err(e) => {
                tracing::warn!(
                    path = %skill_file_path.display(),
                    error = %e,
                    "Failed to read SKILL.md"
                );
            }
        }
    }

    result
}

/// Load the full markdown content of a skill (lazy-loaded on invocation).
pub async fn load_skill_content(skill: &SkillDefinition) -> Result<String, std::io::Error> {
    let skill_file_path = skill.base_dir.join("SKILL.md");
    tokio::fs::read_to_string(skill_file_path).await
}

/// Load, validate, substitute, and wrap a skill for injection.
///
/// This is the single authoritative code path for preparing skill content.
/// Both `SkillTool::call()` and the slash command handler MUST use this
/// function to avoid divergent logic.
///
/// # Validation
/// - `disable_model_invocation`: blocked when `is_model_invocation` is true
/// - `user_invocable`: always enforced
///
/// # Substitutions
/// - `${KEZEN_SKILL_DIR}` → skill base directory
/// - `${KEZEN_SESSION_ID}` → env var `KEZEN_SESSION_ID` (if present)
/// - `${KEZEN_SKILL_ARGS}` → provided `args` (if non-empty)
pub async fn prepare_skill_content(
    skill: &SkillDefinition,
    args: &str,
    is_model_invocation: bool,
) -> Result<String, String> {
    // Validation: respect frontmatter directives.
    if is_model_invocation && skill.frontmatter.disable_model_invocation {
        return Err(format!(
            "Skill '{}' cannot be invoked by the model (disable_model_invocation is set)",
            skill.name
        ));
    }
    // user_invocable only gates slash-command (user) invocations.
    // Model invocations are controlled separately by disable_model_invocation.
    if !is_model_invocation && !skill.frontmatter.user_invocable {
        return Err(format!("Skill '{}' is not user-invocable", skill.name));
    }

    // Load the full content lazily.
    let mut content = load_skill_content(skill).await.map_err(|e| {
        format!("Failed to load skill content: {}", e)
    })?;

    // Variable substitution.
    let base_dir = skill.base_dir.display().to_string();
    content = content.replace("${KEZEN_SKILL_DIR}", &base_dir);

    // TODO: Support ${KEZEN_SESSION_ID} substitution once runtime context
    // (session ID) can be threaded through prepare_skill_content. Currently
    // the env var is never set, making this substitution dead code.

    if !args.is_empty() {
        content = content.replace("${KEZEN_SKILL_ARGS}", args);
    }

    // Wrap in XML tags for the Engine to extract and inject.
    Ok(format!(
        "<skill name=\"{}\" base_dir=\"{}\">\n{}\n</skill>",
        skill.name, base_dir, content
    ))
}

/// Parse YAML-like frontmatter from a SKILL.md file.
///
/// Returns the parsed frontmatter struct and the byte length of the body content
/// (everything after the closing `---`).
pub fn parse_skill_frontmatter(text: &str) -> (SkillFrontmatter, usize) {
    let mut fm = SkillFrontmatter::default();

    if !text.starts_with("---\n") && !text.starts_with("---\r\n") {
        return (fm, text.len());
    }

    let start_idx = text.find('\n').unwrap() + 1;
    let end_relative_idx = match text[start_idx..].find("\n---") {
        Some(i) => i,
        None => return (fm, text.len()),
    };

    let end_idx = start_idx + end_relative_idx;
    let frontmatter = &text[start_idx..end_idx];

    let mut content_start = end_idx + 4;
    if text[content_start..].starts_with("\r\n") {
        content_start += 2;
    } else if text[content_start..].starts_with('\n') {
        content_start += 1;
    }
    let body_len = text.len().saturating_sub(content_start);

    let mut current_array: Option<&mut Vec<String>> = None;

    for line in frontmatter.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        if trimmed.starts_with("- ") {
            if let Some(arr) = &mut current_array {
                let val = trimmed.strip_prefix("- ").unwrap().trim().trim_matches(|c| c == '"' || c == '\'');
                if !val.is_empty() {
                    arr.push(val.to_string());
                }
            }
            continue;
        }

        if let Some((key, value)) = trimmed.split_once(':') {
            let key = key.trim();
            let value = value.trim().trim_matches(|c| c == '"' || c == '\'');

            current_array = None;

            match key {
                "name" => fm.name = if value.is_empty() { None } else { Some(value.to_string()) },
                "description" => fm.description = if value.is_empty() { None } else { Some(value.to_string()) },
                "when_to_use" => fm.when_to_use = if value.is_empty() { None } else { Some(value.to_string()) },
                "model" => fm.model = if value.is_empty() { None } else { Some(value.to_string()) },
                "argument_hint" => fm.argument_hint = if value.is_empty() { None } else { Some(value.to_string()) },
                "disable_model_invocation" => {
                    fm.disable_model_invocation = value.eq_ignore_ascii_case("true");
                }
                "user_invocable" => {
                    fm.user_invocable = !value.eq_ignore_ascii_case("false"); // default true
                }
                "allowed_tools" => {
                    // Support inline YAML arrays: allowed_tools: [bash, file_write]
                    if let Some(inner) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
                        fm.allowed_tools = inner
                            .split(',')
                            .map(|s| s.trim().trim_matches(|c| c == '"' || c == '\'').to_string())
                            .filter(|s| !s.is_empty())
                            .collect();
                    } else {
                        current_array = Some(&mut fm.allowed_tools);
                    }
                }
                "paths" | "files" => {
                    if fm.paths.is_none() {
                        fm.paths = Some(Vec::new());
                    }
                    // Support inline YAML arrays: paths: [src/, tests/]
                    if let Some(inner) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
                        let paths = fm.paths.get_or_insert_with(Vec::new);
                        paths.extend(
                            inner
                                .split(',')
                                .map(|s| s.trim().trim_matches(|c| c == '"' || c == '\'').to_string())
                                .filter(|s| !s.is_empty())
                        );
                    } else {
                        current_array = fm.paths.as_mut();
                    }
                }
                _ => {}
            }
        }
    }

    (fm, body_len)
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ── Frontmatter parsing (pure, sync) ────────────────────────────────────

    #[test]
    fn test_parse_frontmatter_full() {
        let content = "---\n\
name: \"Test Skill\"\n\
description: 'A test skill'\n\
allowed_tools:\n\
  - bash\n\
  - file_write\n\
disable_model_invocation: true\n\
---\n\
Here is the text.\n";

        let (fm, len) = parse_skill_frontmatter(content);
        assert_eq!(fm.name.unwrap(), "Test Skill");
        assert_eq!(fm.description.unwrap(), "A test skill");
        assert_eq!(fm.allowed_tools, vec!["bash", "file_write"]);
        assert!(fm.disable_model_invocation);
        assert!(fm.user_invocable); // default true
        assert_eq!(len, "Here is the text.\n".len());
    }

    #[test]
    fn test_parse_frontmatter_no_frontmatter() {
        let content = "Just text, no frontmatter";
        let (fm, len) = parse_skill_frontmatter(content);
        assert!(fm.name.is_none());
        assert!(fm.description.is_none());
        assert!(fm.allowed_tools.is_empty());
        assert!(fm.user_invocable);
        assert!(!fm.disable_model_invocation);
        assert_eq!(len, content.len());
    }

    #[test]
    fn test_parse_frontmatter_user_invocable_false() {
        let content = "---\n\
user_invocable: false\n\
---\n\
Body text\n";
        let (fm, _) = parse_skill_frontmatter(content);
        assert!(!fm.user_invocable);
    }

    #[test]
    fn test_parse_frontmatter_when_to_use_and_hint() {
        let content = "---\n\
name: deploy\n\
when_to_use: When the user wants to deploy to production\n\
argument_hint: <environment>\n\
---\n\
Instructions here.\n";
        let (fm, _) = parse_skill_frontmatter(content);
        assert_eq!(fm.name.unwrap(), "deploy");
        assert_eq!(fm.when_to_use.unwrap(), "When the user wants to deploy to production");
        assert_eq!(fm.argument_hint.unwrap(), "<environment>");
    }

    #[test]
    fn test_parse_frontmatter_model_override() {
        let content = "---\n\
model: claude-3-haiku\n\
---\n\
Quick task.\n";
        let (fm, _) = parse_skill_frontmatter(content);
        assert_eq!(fm.model.unwrap(), "claude-3-haiku");
    }

    #[test]
    fn test_parse_frontmatter_paths_field() {
        let content = "---\n\
paths:\n\
  - src/\n\
  - tests/\n\
---\n\
Scoped skill.\n";
        let (fm, _) = parse_skill_frontmatter(content);
        let paths = fm.paths.unwrap();
        assert_eq!(paths, vec!["src/", "tests/"]);
    }

    #[test]
    fn test_parse_frontmatter_unclosed_returns_full_length() {
        let content = "---\nname: broken\nno closing delimiter";
        let (fm, len) = parse_skill_frontmatter(content);
        // No closing --- → treat entire text as body
        assert!(fm.name.is_none()); // frontmatter not committed
        assert_eq!(len, content.len());
    }

    #[test]
    fn test_parse_frontmatter_empty_values_are_none() {
        let content = "---\n\
name:\n\
description:\n\
---\n\
Body.\n";
        let (fm, _) = parse_skill_frontmatter(content);
        assert!(fm.name.is_none());
        assert!(fm.description.is_none());
    }

    #[test]
    fn test_parse_frontmatter_comments_and_blanks_ignored() {
        let content = "---\n\
# This is a comment\n\
\n\
name: valid\n\
---\n\
Body.\n";
        let (fm, _) = parse_skill_frontmatter(content);
        assert_eq!(fm.name.unwrap(), "valid");
    }

    #[test]
    fn test_parse_frontmatter_crlf_line_endings() {
        let content = "---\r\nname: crlf\r\n---\r\nBody.\r\n";
        let (fm, len) = parse_skill_frontmatter(content);
        assert_eq!(fm.name.unwrap(), "crlf");
        assert_eq!(len, "Body.\r\n".len());
    }

    // ── Async filesystem tests ──────────────────────────────────────────────

    #[tokio::test]
    async fn test_load_skills_from_dir_empty() {
        let dir = tempfile::tempdir().unwrap();
        let skills = load_skills_from_dir(dir.path(), SkillSource::Project).await;
        assert!(skills.is_empty());
    }

    #[tokio::test]
    async fn test_load_skills_from_dir_with_skill() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("my-skill");
        std::fs::create_dir(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: my-skill\ndescription: Test skill\n---\nInstructions here.\n",
        ).unwrap();

        let skills = load_skills_from_dir(dir.path(), SkillSource::Project).await;
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "my-skill");
        assert_eq!(skills[0].frontmatter.description.as_deref(), Some("Test skill"));
        assert_eq!(skills[0].source, SkillSource::Project);
    }

    #[tokio::test]
    async fn test_load_skills_from_dir_skips_files() {
        let dir = tempfile::tempdir().unwrap();
        // A regular file (not a directory) should be skipped
        std::fs::write(dir.path().join("not-a-dir.md"), "hello").unwrap();

        let skills = load_skills_from_dir(dir.path(), SkillSource::UserGlobal).await;
        assert!(skills.is_empty());
    }

    #[tokio::test]
    async fn test_load_skills_from_dir_skips_dir_without_skill_md() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("no-skill-file");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("README.md"), "not a skill").unwrap();

        let skills = load_skills_from_dir(dir.path(), SkillSource::Project).await;
        assert!(skills.is_empty());
    }

    #[tokio::test]
    async fn test_load_skills_from_dir_multiple_skills() {
        let dir = tempfile::tempdir().unwrap();

        for name in &["alpha", "beta", "gamma"] {
            let skill_dir = dir.path().join(name);
            std::fs::create_dir(&skill_dir).unwrap();
            std::fs::write(
                skill_dir.join("SKILL.md"),
                format!("---\nname: {}\ndescription: Skill {}\n---\nBody.\n", name, name),
            ).unwrap();
        }

        let skills = load_skills_from_dir(dir.path(), SkillSource::UserGlobal).await;
        assert_eq!(skills.len(), 3);
    }

    #[tokio::test]
    async fn test_load_skills_from_nonexistent_dir() {
        let dir = PathBuf::from("/tmp/kezen-test-nonexistent-dir-12345");
        let skills = load_skills_from_dir(&dir, SkillSource::Project).await;
        assert!(skills.is_empty());
    }

    #[tokio::test]
    async fn test_load_skill_content() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("test-content");
        std::fs::create_dir(&skill_dir).unwrap();
        let body = "---\nname: test\n---\nThe actual instructions.\n";
        std::fs::write(skill_dir.join("SKILL.md"), body).unwrap();

        let skill = SkillDefinition {
            name: "test-content".to_string(),
            frontmatter: SkillFrontmatter::default(),
            body_length: 24,
            source: SkillSource::Project,
            base_dir: skill_dir,
        };

        let content = load_skill_content(&skill).await.unwrap();
        assert_eq!(content, body);
    }

    #[tokio::test]
    async fn test_load_skill_content_missing_file() {
        let skill = SkillDefinition {
            name: "ghost".to_string(),
            frontmatter: SkillFrontmatter::default(),
            body_length: 0,
            source: SkillSource::Project,
            base_dir: PathBuf::from("/tmp/kezen-test-ghost-skill-99999"),
        };

        let result = load_skill_content(&skill).await;
        assert!(result.is_err());
    }

    // ── prepare_skill_content ───────────────────────────────────────────────

    #[tokio::test]
    async fn test_prepare_skill_content_success() {
        let dir = tempfile::tempdir().unwrap();
        let body = "---\nname: test\n---\nDo with ${KEZEN_SKILL_DIR} and ${KEZEN_SKILL_ARGS}.\n";
        std::fs::write(dir.path().join("SKILL.md"), body).unwrap();

        let skill = SkillDefinition {
            name: "test".to_string(),
            frontmatter: SkillFrontmatter::default(),
            body_length: 100,
            source: SkillSource::Project,
            base_dir: dir.path().to_path_buf(),
        };

        let result = prepare_skill_content(&skill, "hello", false).await.unwrap();
        assert!(result.contains("<skill name=\"test\""));
        assert!(result.contains("</skill>"));
        assert!(!result.contains("${KEZEN_SKILL_DIR}"));
        assert!(result.contains("hello"));
    }

    #[tokio::test]
    async fn test_prepare_skill_content_blocks_model_invocation() {
        let skill = SkillDefinition {
            name: "locked".to_string(),
            frontmatter: SkillFrontmatter {
                disable_model_invocation: true,
                ..Default::default()
            },
            body_length: 0,
            source: SkillSource::Project,
            base_dir: PathBuf::from("/tmp/test"),
        };

        let result = prepare_skill_content(&skill, "", true).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("disable_model_invocation"));
    }

    #[tokio::test]
    async fn test_prepare_skill_content_allows_slash_when_model_disabled() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("SKILL.md"), "---\nname: x\n---\nBody.\n").unwrap();

        let skill = SkillDefinition {
            name: "x".to_string(),
            frontmatter: SkillFrontmatter {
                disable_model_invocation: true,
                ..Default::default()
            },
            body_length: 10,
            source: SkillSource::Project,
            base_dir: dir.path().to_path_buf(),
        };

        // Slash command invocation (is_model_invocation = false) should NOT be blocked
        let result = prepare_skill_content(&skill, "", false).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_prepare_skill_content_blocks_non_invocable() {
        let skill = SkillDefinition {
            name: "internal".to_string(),
            frontmatter: SkillFrontmatter {
                user_invocable: false,
                ..Default::default()
            },
            body_length: 0,
            source: SkillSource::Project,
            base_dir: PathBuf::from("/tmp/test"),
        };

        let result = prepare_skill_content(&skill, "", false).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not user-invocable"));
    }

    // ── Inline YAML arrays ──────────────────────────────────────────────────

    #[test]
    fn test_parse_frontmatter_inline_array_allowed_tools() {
        let content = "---\n\
allowed_tools: [bash, file_write]\n\
---\n\
Body.\n";
        let (fm, _) = parse_skill_frontmatter(content);
        assert_eq!(fm.allowed_tools, vec!["bash", "file_write"]);
    }

    #[test]
    fn test_parse_frontmatter_inline_array_paths() {
        let content = "---\n\
paths: [\"src/\", \"tests/\"]\n\
---\n\
Body.\n";
        let (fm, _) = parse_skill_frontmatter(content);
        let paths = fm.paths.unwrap();
        assert_eq!(paths, vec!["src/", "tests/"]);
    }

    #[test]
    fn test_parse_frontmatter_inline_array_empty_brackets() {
        let content = "---\n\
allowed_tools: []\n\
---\n\
Body.\n";
        let (fm, _) = parse_skill_frontmatter(content);
        assert!(fm.allowed_tools.is_empty());
    }

    // ── files: alias for paths: ─────────────────────────────────────────────

    #[test]
    fn test_parse_frontmatter_files_alias() {
        let content = "---\n\
files:\n\
  - src/\n\
  - tests/\n\
---\n\
Scoped skill.\n";
        let (fm, _) = parse_skill_frontmatter(content);
        let paths = fm.paths.unwrap();
        assert_eq!(paths, vec!["src/", "tests/"]);
    }

    // ── C-1: model invocation bypasses user_invocable ────────────────────────

    #[tokio::test]
    async fn test_prepare_skill_content_model_can_call_non_user_invocable() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("SKILL.md"), "---\nname: auto\n---\nAuto instructions.\n").unwrap();

        let skill = SkillDefinition {
            name: "auto".to_string(),
            frontmatter: SkillFrontmatter {
                user_invocable: false,
                ..Default::default()
            },
            body_length: 20,
            source: SkillSource::Project,
            base_dir: dir.path().to_path_buf(),
        };

        // Model invocation (is_model_invocation=true) bypasses user_invocable check
        let result = prepare_skill_content(&skill, "", true).await;
        assert!(result.is_ok(), "Model should bypass user_invocable: {}", result.unwrap_err());
    }
}
