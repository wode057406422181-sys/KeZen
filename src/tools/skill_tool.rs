use std::sync::Arc;
use async_trait::async_trait;
use serde_json::json;

use crate::constants::engine::SKILL_TOOL_NAME;
use crate::permissions::PermissionResult;
use crate::skills::registry::SkillRegistry;
use crate::skills::loader::prepare_skill_content;
use crate::tools::{Tool, ToolResult};

pub struct SkillTool {
    registry: Arc<SkillRegistry>,
}

impl SkillTool {
    pub fn new(registry: Arc<SkillRegistry>) -> Self {
        Self { registry }
    }
}

/// Normalize a skill name by stripping a leading `/` if present.
///
/// LLMs sometimes prepend a slash when the user says "/commit"; we
/// tolerate this rather than failing with "Unknown skill: /commit".
fn normalize_skill_name(raw: &str) -> &str {
    let trimmed = raw.trim();
    trimmed.strip_prefix('/').unwrap_or(trimmed)
}

#[async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &str {
        SKILL_TOOL_NAME
    }

    fn description(&self) -> &str {
        // Rich multi-line prompt that guides the LLM on when and how to use
        // this tool. Modeled after industry best practices for skill invocation.
        concat!(
            "Execute a skill within the main conversation.\n",
            "\n",
            "When users ask you to perform tasks, check if any of the available skills match. ",
            "Skills provide specialized capabilities and domain knowledge.\n",
            "\n",
            "When users reference a \"slash command\" or \"/something\" ",
            "(e.g. \"/commit\", \"/review-pr\"), they are referring to a skill. ",
            "Use this tool to invoke it.\n",
            "\n",
            "How to invoke:\n",
            "- Use this tool with the skill name and optional arguments.\n",
            "- Examples:\n",
            "  - `skill: \"commit\"` — invoke the commit skill\n",
            "  - `skill: \"commit\", args: \"-m 'Fix bug'\"` — invoke with arguments\n",
            "  - `skill: \"review-pr\", args: \"123\"` — invoke with arguments\n",
            "\n",
            "Important:\n",
            "- Available skills are listed in the system prompt under <skills>.\n",
            "- When a skill matches the user's request, this is a BLOCKING REQUIREMENT: ",
            "invoke the relevant Skill tool BEFORE generating any other response about the task.\n",
            "- NEVER mention a skill without actually calling this tool.\n",
            "- Do not invoke a skill that is already running.\n",
            "- Do not use this tool for built-in CLI commands (like /help, /clear, etc.).\n",
            "- If you see a <skill> tag in the current conversation turn, the skill has ",
            "ALREADY been loaded — follow the instructions directly instead of calling this tool again.\n",
        )
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "skill": {
                    "type": "string",
                    "description": "The skill name to execute (e.g. \"commit\", \"review-pr\"). A leading / is tolerated and will be stripped."
                },
                "args": {
                    "type": "string",
                    "description": "Optional arguments for the skill."
                }
            },
            "required": ["skill"]
        })
    }

    async fn call(&self, input: serde_json::Value) -> ToolResult {
        let raw_skill = match input.get("skill").and_then(|v| v.as_str()) {
            Some(name) => name,
            None => {
                tracing::warn!(tool = SKILL_TOOL_NAME, "Missing or invalid 'skill' argument in input");
                return ToolResult::err("Missing or invalid 'skill' argument".into());
            }
        };

        // Normalize: strip leading `/` if present.
        let skill_name = normalize_skill_name(raw_skill);
        if skill_name.is_empty() {
            tracing::warn!(tool = SKILL_TOOL_NAME, raw = %raw_skill, "Empty skill name after normalization");
            return ToolResult::err(format!("Invalid skill format: {}", raw_skill));
        }

        let args = input.get("args").and_then(|v| v.as_str()).unwrap_or("");

        let skill = match self.registry.get(skill_name) {
            Some(s) => s,
            None => {
                tracing::warn!(tool = SKILL_TOOL_NAME, skill = %skill_name, "Unknown skill requested");
                return ToolResult::err(format!("Unknown skill: {}", skill_name));
            }
        };

        // Delegate to the shared prepare_skill_content() for validation,
        // loading, substitution, and wrapping. is_model_invocation = true
        // because the model is calling this tool.
        match prepare_skill_content(skill, args, true).await {
            Ok(wrapped) => {
                tracing::debug!(
                    tool = SKILL_TOOL_NAME,
                    skill = %skill_name,
                    content_bytes = wrapped.len(),
                    "Skill content loaded and wrapped"
                );
                ToolResult::ok(wrapped)
            }
            Err(msg) => {
                tracing::warn!(
                    tool = SKILL_TOOL_NAME,
                    skill = %skill_name,
                    path = %skill.base_dir.display(),
                    error = %msg,
                    "Skill preparation failed"
                );
                ToolResult::err(msg)
            }
        }
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        // Loading a skill markdown is read-only (side effects happen when the
        // model actually follows the instructions).
        true
    }

    async fn check_permissions(&self, input: &serde_json::Value) -> PermissionResult {
        let skill_name = match input.get("skill").and_then(|s| s.as_str()) {
            Some(n) => normalize_skill_name(n),
            None => return PermissionResult::Passthrough,
        };

        if let Some(skill) = self.registry.get(skill_name)
            && !skill.frontmatter.allowed_tools.is_empty() {
                tracing::debug!(
                    tool = SKILL_TOOL_NAME,
                    skill = %skill_name,
                    allowed_tools = ?skill.frontmatter.allowed_tools,
                    "Skill requires advanced permissions"
                );
                return PermissionResult::Ask {
                    message: format!("Skill '{}' requests advanced permissions to run tools: {:?}", skill_name, skill.frontmatter.allowed_tools),
                };
        }
        PermissionResult::Passthrough
    }

    fn permission_description(&self, input: &serde_json::Value) -> String {
        let raw = input.get("skill").and_then(|v| v.as_str()).unwrap_or("unknown");
        format!("Execute skill: {}", normalize_skill_name(raw))
    }

    fn permission_suggestion(&self, input: &serde_json::Value) -> Option<String> {
        input.get("skill").and_then(|v| v.as_str()).map(|s| normalize_skill_name(s).to_string())
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::types::{SkillDefinition, SkillFrontmatter, SkillSource};
    use std::path::PathBuf;

    fn make_registry_with_skill(dir: &std::path::Path) -> Arc<SkillRegistry> {
        let mut reg = SkillRegistry::new();
        reg.register(SkillDefinition {
            name: "test-skill".to_string(),
            frontmatter: SkillFrontmatter {
                description: Some("A test skill".to_string()),
                ..Default::default()
            },
            body_length: 100,
            source: SkillSource::Project,
            base_dir: dir.to_path_buf(),
        });
        Arc::new(reg)
    }

    fn make_registry_with_restricted_skill(dir: &std::path::Path) -> Arc<SkillRegistry> {
        let mut reg = SkillRegistry::new();
        reg.register(SkillDefinition {
            name: "restricted".to_string(),
            frontmatter: SkillFrontmatter {
                description: Some("Restricted skill".to_string()),
                allowed_tools: vec!["bash".to_string(), "file_write".to_string()],
                ..Default::default()
            },
            body_length: 100,
            source: SkillSource::Project,
            base_dir: dir.to_path_buf(),
        });
        Arc::new(reg)
    }

    fn make_registry_with_disabled_skill() -> Arc<SkillRegistry> {
        let mut reg = SkillRegistry::new();
        reg.register(SkillDefinition {
            name: "disabled".to_string(),
            frontmatter: SkillFrontmatter {
                description: Some("Disabled skill".to_string()),
                disable_model_invocation: true,
                ..Default::default()
            },
            body_length: 100,
            source: SkillSource::Project,
            base_dir: PathBuf::from("/tmp/test-disabled"),
        });
        Arc::new(reg)
    }

    fn make_registry_with_non_invocable_skill() -> Arc<SkillRegistry> {
        let mut reg = SkillRegistry::new();
        reg.register(SkillDefinition {
            name: "internal".to_string(),
            frontmatter: SkillFrontmatter {
                description: Some("Internal only".to_string()),
                user_invocable: false,
                ..Default::default()
            },
            body_length: 100,
            source: SkillSource::Project,
            base_dir: PathBuf::from("/tmp/test-internal"),
        });
        Arc::new(reg)
    }

    // ── normalize_skill_name ────────────────────────────────────────────────

    #[test]
    fn test_normalize_strips_leading_slash() {
        assert_eq!(normalize_skill_name("/commit"), "commit");
    }

    #[test]
    fn test_normalize_preserves_bare_name() {
        assert_eq!(normalize_skill_name("commit"), "commit");
    }

    #[test]
    fn test_normalize_trims_whitespace() {
        assert_eq!(normalize_skill_name("  /deploy  "), "deploy");
    }

    #[test]
    fn test_normalize_double_slash() {
        // Only strip one leading slash
        assert_eq!(normalize_skill_name("//weird"), "/weird");
    }

    // ── Tool metadata ───────────────────────────────────────────────────────

    #[test]
    fn test_name() {
        let tool = SkillTool::new(Arc::new(SkillRegistry::new()));
        assert_eq!(tool.name(), SKILL_TOOL_NAME);
    }

    #[test]
    fn test_description_contains_blocking_requirement() {
        let tool = SkillTool::new(Arc::new(SkillRegistry::new()));
        assert!(tool.description().contains("BLOCKING REQUIREMENT"));
    }

    #[test]
    fn test_description_contains_invocation_examples() {
        let tool = SkillTool::new(Arc::new(SkillRegistry::new()));
        let desc = tool.description();
        assert!(desc.contains("skill: \"commit\""));
        assert!(desc.contains("skill: \"review-pr\""));
    }

    #[test]
    fn test_description_warns_about_already_loaded() {
        let tool = SkillTool::new(Arc::new(SkillRegistry::new()));
        assert!(tool.description().contains("<skill>"));
        assert!(tool.description().contains("ALREADY been loaded"));
    }

    #[test]
    fn test_input_schema_has_required_skill() {
        let tool = SkillTool::new(Arc::new(SkillRegistry::new()));
        let schema = tool.input_schema();
        let required = schema.get("required").unwrap().as_array().unwrap();
        assert!(required.iter().any(|v| v.as_str() == Some("skill")));
    }

    #[test]
    fn test_is_read_only() {
        let tool = SkillTool::new(Arc::new(SkillRegistry::new()));
        assert!(tool.is_read_only(&json!({"skill": "anything"})));
    }

    // ── call() ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_call_missing_skill_arg() {
        let tool = SkillTool::new(Arc::new(SkillRegistry::new()));
        let result = tool.call(json!({})).await;
        assert!(result.is_error);
        assert!(result.content.contains("Missing"));
    }

    #[tokio::test]
    async fn test_call_empty_skill_name() {
        let tool = SkillTool::new(Arc::new(SkillRegistry::new()));
        let result = tool.call(json!({"skill": "/"})).await;
        assert!(result.is_error);
        assert!(result.content.contains("Invalid skill format"));
    }

    #[tokio::test]
    async fn test_call_unknown_skill() {
        let tool = SkillTool::new(Arc::new(SkillRegistry::new()));
        let result = tool.call(json!({"skill": "nonexistent"})).await;
        assert!(result.is_error);
        assert!(result.content.contains("Unknown skill"));
    }

    #[tokio::test]
    async fn test_call_strips_leading_slash() {
        let dir = tempfile::tempdir().unwrap();
        let body = "---\nname: test-skill\n---\nDo the thing.\n";
        std::fs::write(dir.path().join("SKILL.md"), body).unwrap();

        let registry = make_registry_with_skill(dir.path());
        let tool = SkillTool::new(registry);
        // Invoke with leading slash — should still find "test-skill"
        let result = tool.call(json!({"skill": "/test-skill"})).await;
        assert!(!result.is_error, "Leading slash should be stripped: {}", result.content);
    }

    #[tokio::test]
    async fn test_call_success() {
        let dir = tempfile::tempdir().unwrap();
        let body = "---\nname: test-skill\n---\nDo the thing with ${KEZEN_SKILL_DIR}.\n";
        std::fs::write(dir.path().join("SKILL.md"), body).unwrap();

        let registry = make_registry_with_skill(dir.path());
        let tool = SkillTool::new(registry);
        let result = tool.call(json!({"skill": "test-skill"})).await;

        assert!(!result.is_error);
        assert!(result.content.contains("<skill name=\"test-skill\""));
        assert!(result.content.contains("</skill>"));
        assert!(!result.content.contains("${KEZEN_SKILL_DIR}"));
        assert!(result.content.contains(&dir.path().display().to_string()));
    }

    #[tokio::test]
    async fn test_call_with_args() {
        let dir = tempfile::tempdir().unwrap();
        let body = "---\nname: test-skill\n---\nRun ${KEZEN_SKILL_ARGS}.\n";
        std::fs::write(dir.path().join("SKILL.md"), body).unwrap();

        let registry = make_registry_with_skill(dir.path());
        let tool = SkillTool::new(registry);
        let result = tool.call(json!({"skill": "test-skill", "args": "deploy staging"})).await;

        assert!(!result.is_error);
        assert!(result.content.contains("Run deploy staging."));
    }

    #[tokio::test]
    async fn test_call_disable_model_invocation() {
        let tool = SkillTool::new(make_registry_with_disabled_skill());
        let result = tool.call(json!({"skill": "disabled"})).await;
        assert!(result.is_error);
        assert!(result.content.contains("disable_model_invocation"));
    }

    #[tokio::test]
    async fn test_call_non_invocable_model_can_call() {
        // Model invocations (via SkillTool) bypass user_invocable: false
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("SKILL.md"), "---\nname: internal\n---\nInternal only.\n").unwrap();

        let mut reg = SkillRegistry::new();
        reg.register(SkillDefinition {
            name: "internal".to_string(),
            frontmatter: SkillFrontmatter {
                description: Some("Internal only".to_string()),
                user_invocable: false,
                ..Default::default()
            },
            body_length: 15,
            source: SkillSource::Project,
            base_dir: dir.path().to_path_buf(),
        });
        let tool = SkillTool::new(Arc::new(reg));
        let result = tool.call(json!({"skill": "internal"})).await;
        assert!(!result.is_error, "Model should call non-user-invocable skill: {}", result.content);
        assert!(result.content.contains("<skill name=\"internal\""));
    }

    #[tokio::test]
    async fn test_call_file_missing() {
        let tool = SkillTool::new(Arc::new({
            let mut reg = SkillRegistry::new();
            reg.register(SkillDefinition {
                name: "ghost".to_string(),
                frontmatter: SkillFrontmatter::default(),
                body_length: 0,
                source: SkillSource::Project,
                base_dir: PathBuf::from("/tmp/kezen-test-ghost-999"),
            });
            reg
        }));

        let result = tool.call(json!({"skill": "ghost"})).await;
        assert!(result.is_error);
        assert!(result.content.contains("Failed to load skill content"));
    }

    // ── check_permissions ───────────────────────────────────────────────────

    #[tokio::test]
    async fn test_check_permissions_passthrough_for_normal_skill() {
        let dir = tempfile::tempdir().unwrap();
        let registry = make_registry_with_skill(dir.path());
        let tool = SkillTool::new(registry);

        let perm = tool.check_permissions(&json!({"skill": "test-skill"})).await;
        assert!(matches!(perm, PermissionResult::Passthrough));
    }

    #[tokio::test]
    async fn test_check_permissions_ask_for_restricted_skill() {
        let dir = tempfile::tempdir().unwrap();
        let registry = make_registry_with_restricted_skill(dir.path());
        let tool = SkillTool::new(registry);

        let perm = tool.check_permissions(&json!({"skill": "restricted"})).await;
        match perm {
            PermissionResult::Ask { message } => {
                assert!(message.contains("restricted"));
                assert!(message.contains("bash"));
            }
            _ => panic!("Expected PermissionResult::Ask"),
        }
    }

    #[tokio::test]
    async fn test_check_permissions_strips_slash() {
        let dir = tempfile::tempdir().unwrap();
        let registry = make_registry_with_restricted_skill(dir.path());
        let tool = SkillTool::new(registry);

        // Should find the skill even with leading /
        let perm = tool.check_permissions(&json!({"skill": "/restricted"})).await;
        assert!(matches!(perm, PermissionResult::Ask { .. }));
    }

    #[tokio::test]
    async fn test_check_permissions_passthrough_unknown_skill() {
        let tool = SkillTool::new(Arc::new(SkillRegistry::new()));
        let perm = tool.check_permissions(&json!({"skill": "nope"})).await;
        assert!(matches!(perm, PermissionResult::Passthrough));
    }

    #[tokio::test]
    async fn test_check_permissions_passthrough_missing_arg() {
        let tool = SkillTool::new(Arc::new(SkillRegistry::new()));
        let perm = tool.check_permissions(&json!({})).await;
        assert!(matches!(perm, PermissionResult::Passthrough));
    }

    // ── permission metadata ─────────────────────────────────────────────────

    #[test]
    fn test_permission_description() {
        let tool = SkillTool::new(Arc::new(SkillRegistry::new()));
        assert_eq!(tool.permission_description(&json!({"skill": "commit"})), "Execute skill: commit");
    }

    #[test]
    fn test_permission_description_strips_slash() {
        let tool = SkillTool::new(Arc::new(SkillRegistry::new()));
        assert_eq!(tool.permission_description(&json!({"skill": "/commit"})), "Execute skill: commit");
    }

    #[test]
    fn test_permission_description_missing_skill() {
        let tool = SkillTool::new(Arc::new(SkillRegistry::new()));
        assert_eq!(tool.permission_description(&json!({})), "Execute skill: unknown");
    }

    #[test]
    fn test_permission_suggestion() {
        let tool = SkillTool::new(Arc::new(SkillRegistry::new()));
        assert_eq!(tool.permission_suggestion(&json!({"skill": "deploy"})), Some("deploy".to_string()));
        assert_eq!(tool.permission_suggestion(&json!({})), None);
    }

    #[test]
    fn test_permission_suggestion_strips_slash() {
        let tool = SkillTool::new(Arc::new(SkillRegistry::new()));
        assert_eq!(tool.permission_suggestion(&json!({"skill": "/deploy"})), Some("deploy".to_string()));
    }
}
