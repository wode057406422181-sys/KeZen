use indexmap::IndexMap;
use crate::constants::defaults::MAX_LISTING_DESC_CHARS;
use crate::skills::types::SkillDefinition;

/// Central registry for all loaded skills.
#[derive(Debug, Default)]
pub struct SkillRegistry {
    skills: IndexMap<String, SkillDefinition>,
}

impl SkillRegistry {
    pub fn new() -> Self {
        Self {
            skills: IndexMap::new(),
        }
    }

    /// Register a skill definition. Overwrites any existing skill with the same name.
    pub fn register(&mut self, skill: SkillDefinition) {
        tracing::debug!(
            skill = %skill.name,
            source = ?skill.source,
            "Skill registered"
        );
        self.skills.insert(skill.name.clone(), skill);
    }

    pub fn get(&self, name: &str) -> Option<&SkillDefinition> {
        self.skills.get(name)
    }

    pub fn find(&self, name: &str) -> Option<&SkillDefinition> {
        self.skills.get(name)
    }

    pub fn all(&self) -> &IndexMap<String, SkillDefinition> {
        &self.skills
    }

    pub fn len(&self) -> usize {
        self.skills.len()
    }

    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    /// Format the combined description for a single skill entry.
    ///
    /// Merges `description` and `when_to_use` into one string, capped at
    /// [`MAX_LISTING_DESC_CHARS`] to avoid wasting context-window tokens on
    /// verbose entries. Full content is loaded lazily on invocation.
    fn format_entry_description(skill: &SkillDefinition) -> String {
        let desc = skill.frontmatter.description.as_deref().unwrap_or("No description provided");
        let combined = match &skill.frontmatter.when_to_use {
            Some(when) => format!("{} - {}", desc, when),
            None => desc.to_string(),
        };

        let char_count = combined.chars().count();
        if char_count > MAX_LISTING_DESC_CHARS {
            let truncated: String = combined.chars().take(MAX_LISTING_DESC_CHARS - 1).collect();
            format!("{}…", truncated)
        } else {
            combined
        }
    }

    /// Format skill listing for system prompt injection.
    ///
    /// Uses a two-tier degradation strategy when the listing exceeds `budget_chars`:
    /// 1. **Truncate descriptions** — shorten each entry description proportionally.
    /// 2. **Names only** — if even truncated descriptions don't fit, list only names.
    ///
    /// The default budget is [`DEFAULT_SKILL_BUDGET_CHARS`] (1% of 200k context × 4 chars/token).
    pub fn format_listing(&self, budget_chars: usize) -> String {
        if self.skills.is_empty() {
            return String::new();
        }

        // First pass: build full entries
        let full_entries: Vec<(String, String)> = self.skills.iter()
            .map(|(name, skill)| {
                let desc = Self::format_entry_description(skill);
                let mut entry = format!("- {}: {}", name, desc);
                if let Some(hint) = &skill.frontmatter.argument_hint {
                    entry.push_str(&format!(" [args: {}]", hint));
                }
                (name.clone(), entry)
            })
            .collect();

        let full_total: usize = full_entries.iter().map(|(_, e)| e.len() + 1).sum();

        // Happy path: everything fits
        if full_total <= budget_chars {
            return full_entries.iter().map(|(_, e)| e.as_str()).collect::<Vec<_>>().join("\n");
        }

        // Tier 1: truncate descriptions proportionally
        let name_overhead: usize = full_entries.iter()
            .map(|(name, _)| name.len() + 4) // "- " + ": "
            .sum::<usize>()
            + full_entries.len(); // newlines
        let available_for_descs = budget_chars.saturating_sub(name_overhead);
        let max_desc_len = available_for_descs / full_entries.len();

        if max_desc_len >= 20 {
            tracing::debug!(
                skills = full_entries.len(),
                budget = budget_chars,
                max_desc_len,
                "Skill listing: truncating descriptions to fit budget"
            );
            let truncated: Vec<String> = self.skills.iter()
                .map(|(name, skill)| {
                    let desc = Self::format_entry_description(skill);
                    if desc.chars().count() > max_desc_len {
                        let short: String = desc.chars().take(max_desc_len.saturating_sub(1)).collect();
                        format!("- {}: {}…", name, short)
                    } else {
                        format!("- {}: {}", name, desc)
                    }
                })
                .collect();
            return truncated.join("\n");
        }

        // Tier 2: names only
        tracing::debug!(
            skills = full_entries.len(),
            budget = budget_chars,
            "Skill listing: extreme budget — names only"
        );
        full_entries.iter().map(|(name, _)| format!("- {}", name)).collect::<Vec<_>>().join("\n")
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::defaults::DEFAULT_SKILL_BUDGET_CHARS;
    use crate::skills::types::{SkillFrontmatter, SkillSource};
    use std::path::PathBuf;

    fn make_skill(name: &str, desc: Option<&str>) -> SkillDefinition {
        SkillDefinition {
            name: name.to_string(),
            frontmatter: SkillFrontmatter {
                description: desc.map(String::from),
                ..Default::default()
            },
            content_length: 100,
            source: SkillSource::Project,
            base_dir: PathBuf::from("/tmp/test"),
        }
    }

    fn make_skill_with_hints(name: &str, desc: &str, when: &str, hint: &str) -> SkillDefinition {
        SkillDefinition {
            name: name.to_string(),
            frontmatter: SkillFrontmatter {
                description: Some(desc.to_string()),
                when_to_use: Some(when.to_string()),
                argument_hint: Some(hint.to_string()),
                ..Default::default()
            },
            content_length: 100,
            source: SkillSource::UserGlobal,
            base_dir: PathBuf::from("/tmp/test"),
        }
    }

    #[test]
    fn test_registry_new_is_empty() {
        let reg = SkillRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
    }

    #[test]
    fn test_registry_register_and_get() {
        let mut reg = SkillRegistry::new();
        reg.register(make_skill("commit", Some("Generate a commit message")));

        assert_eq!(reg.len(), 1);
        assert!(!reg.is_empty());

        let skill = reg.get("commit").unwrap();
        assert_eq!(skill.name, "commit");
        assert_eq!(skill.frontmatter.description.as_deref(), Some("Generate a commit message"));
    }

    #[test]
    fn test_registry_get_nonexistent() {
        let reg = SkillRegistry::new();
        assert!(reg.get("nonexistent").is_none());
    }

    #[test]
    fn test_registry_overwrite_same_name() {
        let mut reg = SkillRegistry::new();
        reg.register(make_skill("deploy", Some("v1")));
        reg.register(make_skill("deploy", Some("v2")));

        assert_eq!(reg.len(), 1);
        assert_eq!(reg.get("deploy").unwrap().frontmatter.description.as_deref(), Some("v2"));
    }

    #[test]
    fn test_registry_find_delegates_to_get() {
        let mut reg = SkillRegistry::new();
        reg.register(make_skill("test", Some("Test skill")));

        assert!(reg.find("test").is_some());
        assert!(reg.find("missing").is_none());
    }

    #[test]
    fn test_registry_all_returns_all() {
        let mut reg = SkillRegistry::new();
        reg.register(make_skill("a", Some("first")));
        reg.register(make_skill("b", Some("second")));
        reg.register(make_skill("c", Some("third")));

        let all = reg.all();
        assert_eq!(all.len(), 3);
        assert!(all.contains_key("a"));
        assert!(all.contains_key("b"));
        assert!(all.contains_key("c"));
    }

    // ── format_listing ──────────────────────────────────────────────────────

    #[test]
    fn test_format_listing_basic() {
        let mut reg = SkillRegistry::new();
        reg.register(make_skill("commit", Some("Generate a commit message")));
        reg.register(make_skill("deploy", Some("Deploy to production")));

        let listing = reg.format_listing(DEFAULT_SKILL_BUDGET_CHARS);
        assert!(listing.contains("commit"));
        assert!(listing.contains("Generate a commit message"));
        assert!(listing.contains("deploy"));
        assert!(listing.contains("Deploy to production"));
    }

    #[test]
    fn test_format_listing_merges_when_to_use() {
        let mut reg = SkillRegistry::new();
        reg.register(make_skill_with_hints("deploy", "Deploy app", "When deploying", "<env>"));

        let listing = reg.format_listing(DEFAULT_SKILL_BUDGET_CHARS);
        // description and when_to_use are now merged with " - "
        assert!(listing.contains("Deploy app - When deploying"));
        assert!(listing.contains("[args: <env>]"));
    }

    #[test]
    fn test_format_listing_no_description() {
        let mut reg = SkillRegistry::new();
        reg.register(make_skill("bare", None));

        let listing = reg.format_listing(DEFAULT_SKILL_BUDGET_CHARS);
        assert!(listing.contains("No description provided"));
    }

    #[test]
    fn test_format_listing_truncation_tier1() {
        let mut reg = SkillRegistry::new();
        // Register skills with moderately long descriptions
        for i in 0..20 {
            reg.register(make_skill(
                &format!("skill-{:03}", i),
                Some(&format!("Description for skill {} that is moderately detailed", i)),
            ));
        }

        // Budget that forces description truncation (tier 1) but NOT names-only (tier 2).
        // name_overhead ~ 20 * 13 + 20 = 280, so budget 700 leaves ~420 for descs, or ~21 chars each.
        let listing = reg.format_listing(700);
        // Should still contain skill names
        assert!(listing.contains("skill-000"));
        // Descriptions should be truncated (contain ellipsis)
        assert!(listing.contains('…'));
    }

    #[test]
    fn test_format_listing_truncation_tier2_names_only() {
        let mut reg = SkillRegistry::new();
        for i in 0..100 {
            reg.register(make_skill(
                &format!("skill-{:03}", i),
                Some(&format!("Very long description that will never fit in any reasonable budget for skill number {}", i)),
            ));
        }

        // Extremely small budget — should fall to names-only
        let listing = reg.format_listing(100);
        assert!(listing.contains("- skill-000"));
        // Should NOT contain descriptions
        assert!(!listing.contains("Very long description"));
    }

    #[test]
    fn test_format_listing_empty_registry() {
        let reg = SkillRegistry::new();
        let listing = reg.format_listing(DEFAULT_SKILL_BUDGET_CHARS);
        assert!(listing.is_empty());
    }

    #[test]
    fn test_format_entry_description_truncates_long() {
        let skill = SkillDefinition {
            name: "test".to_string(),
            frontmatter: SkillFrontmatter {
                description: Some("x".repeat(300)),
                ..Default::default()
            },
            content_length: 100,
            source: SkillSource::Project,
            base_dir: PathBuf::from("/tmp"),
        };

        let desc = SkillRegistry::format_entry_description(&skill);
        assert!(desc.chars().count() <= MAX_LISTING_DESC_CHARS);
        assert!(desc.ends_with('…'));
    }

    #[test]
    fn test_format_entry_description_merges_when_to_use() {
        let skill = SkillDefinition {
            name: "test".to_string(),
            frontmatter: SkillFrontmatter {
                description: Some("Do something".to_string()),
                when_to_use: Some("When the user asks to do it".to_string()),
                ..Default::default()
            },
            content_length: 100,
            source: SkillSource::Project,
            base_dir: PathBuf::from("/tmp"),
        };

        let desc = SkillRegistry::format_entry_description(&skill);
        assert_eq!(desc, "Do something - When the user asks to do it");
    }
}
