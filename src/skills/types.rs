use std::path::PathBuf;

/// Source of the skill definition
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillSource {
    UserGlobal,
    Project,
    #[allow(dead_code)] // TODO: Bundled skills for built-in skill packs
    Bundled,
}

/// YAML frontmatter fields parsed from SKILL.md
#[derive(Debug, Clone)]
pub struct SkillFrontmatter {
    pub name: Option<String>,
    pub description: Option<String>,
    pub when_to_use: Option<String>,
    pub allowed_tools: Vec<String>,
    pub model: Option<String>,
    pub disable_model_invocation: bool,
    pub user_invocable: bool,
    pub paths: Option<Vec<String>>,
    pub argument_hint: Option<String>,
}

/// Skills are user-invocable by default (`user_invocable: true`).
impl Default for SkillFrontmatter {
    fn default() -> Self {
        Self {
            name: None,
            description: None,
            when_to_use: None,
            allowed_tools: Vec::new(),
            model: None,
            disable_model_invocation: false,
            user_invocable: true,
            paths: None,
            argument_hint: None,
        }
    }
}

/// A loaded skill definition (frontmatter + metadata)
#[derive(Debug, Clone)]
pub struct SkillDefinition {
    pub name: String,
    pub frontmatter: SkillFrontmatter,
    /// Byte length of the skill body (content after the frontmatter delimiter).
    pub body_length: usize,
    pub source: SkillSource,
    pub base_dir: PathBuf,
}
