pub mod safety;

use serde::{Deserialize, Serialize};

/// Risk level for permission prompts shown to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RiskLevel {
    Low,
    Medium,
    /// TODO: Produce High for extremely dangerous operations (e.g. rm -rf /, format disk)
    High,
}

/// The global permission mode for the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionMode {
    /// Default: read-only tools auto-allow, write tools prompt.
    Default,
    /// Auto-approve file edits within the working directory; Bash still prompts.
    AcceptEdits,
    /// Bypass all permission checks (`--yes` / `-y`).
    DontAsk,
}

/// Result of a tool-level permission check (returned by `Tool::check_permissions`).
#[derive(Debug, Clone)]
pub enum PermissionResult {
    /// Unconditionally allow this invocation.
    Allow,
    /// Unconditionally deny this invocation.
    Deny { message: String },
    /// Need user approval.
    Ask { message: String },
    /// Tool has no opinion; defer to the general pipeline.
    Passthrough,
}

/// A permission rule: (tool_name, optional content pattern).
///
/// Examples:
/// - `("Bash", None)` — matches all Bash invocations.
/// - `("Bash", Some("git commit:*"))` — matches Bash commands starting with `git commit`.
/// - `("FileWrite", Some("src/**"))` — matches FileWrite to files under `src/`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PermissionRuleValue {
    pub tool_name: String,
    pub rule_content: Option<String>,
}

/// The outcome of the full permission pipeline (Engine-facing).
#[derive(Debug, Clone)]
pub enum PermissionDecision {
    Allow,
    Deny { message: String },
    NeedsApproval {
        tool_name: String,
        description: String,
        risk_level: RiskLevel,
        /// Suggested rule content for "always allow" (e.g. `"git commit:*"`).
        suggestion: Option<String>,
    },
}

/// Session-scoped permission state: mode + accumulated allow/deny/ask rules.
pub struct PermissionState {
    pub mode: PermissionMode,
    allow_rules: Vec<PermissionRuleValue>,
    deny_rules: Vec<PermissionRuleValue>,
}

impl PermissionState {
    pub fn new(mode: PermissionMode) -> Self {
        Self {
            mode,
            allow_rules: Vec::new(),
            deny_rules: Vec::new(),
        }
    }

    // ── Rule management ──────────────────────────────────────────────

    /// Add an allow rule (e.g. user chose "always allow").
    pub fn add_allow_rule(&mut self, tool_name: &str, content: Option<&str>) {
        let rule = PermissionRuleValue {
            tool_name: tool_name.to_string(),
            rule_content: content.map(|s| s.to_string()),
        };
        if !self.allow_rules.contains(&rule) {
            self.allow_rules.push(rule);
        }
    }

    /// Add a deny rule.
    #[allow(dead_code)] // TODO: Expose via /deny command or config file
    pub fn add_deny_rule(&mut self, tool_name: &str, content: Option<&str>) {
        let rule = PermissionRuleValue {
            tool_name: tool_name.to_string(),
            rule_content: content.map(|s| s.to_string()),
        };
        if !self.deny_rules.contains(&rule) {
            self.deny_rules.push(rule);
        }
    }

    // ── Pipeline ─────────────────────────────────────────────────────

    /// Run the full permission pipeline for a tool invocation.
    ///
    /// Pipeline order:
    /// 1. Deny rules  → deny
    /// 2. Tool self-check (`check_permissions`)
    ///    - Deny  → deny
    ///    - Allow → allow
    ///    - Ask   → ask (with tool message)
    /// 3. DontAsk mode → allow everything
    /// 4. AcceptEdits mode → allow file tools in working dir
    /// 5. Allow rules (with content matching via `permission_matcher`)
    /// 6. Read-only tools → allow
    /// 7. Default → ask
    #[allow(clippy::too_many_arguments)]
    pub fn check(
        &self,
        tool_name: &str,
        input: &serde_json::Value,
        tool_check: &PermissionResult,
        is_read_only: bool,
        is_file_tool: bool,
        desc: String,
        permission_matcher: Option<&dyn Fn(&str) -> bool>,
        suggestion: Option<String>,
    ) -> PermissionDecision {
        // Step 1: Deny rules
        if self.matches_rules(&self.deny_rules, tool_name, input, permission_matcher) {
            return PermissionDecision::Deny {
                message: format!("Permission to use {} has been denied by a rule.", tool_name),
            };
        }

        // Step 2: Tool self-check
        match tool_check {
            PermissionResult::Deny { message } => {
                return PermissionDecision::Deny {
                    message: message.clone(),
                };
            }
            PermissionResult::Allow => {
                return PermissionDecision::Allow;
            }
            PermissionResult::Ask { message } => {
                // Tool says ask — but check if DontAsk or allow rules override
                // (fall through to step 3+)
                // However, if the tool explicitly asks (e.g. dangerous path), honour it
                // by checking if mode would override
                if self.mode == PermissionMode::DontAsk {
                    return PermissionDecision::Allow;
                }
                // Check allow rules before prompting
                if self.matches_rules(&self.allow_rules, tool_name, input, permission_matcher) {
                    return PermissionDecision::Allow;
                }
                return PermissionDecision::NeedsApproval {
                    tool_name: tool_name.to_string(),
                    description: message.clone(),
                    risk_level: RiskLevel::Medium,
                    suggestion,
                };
            }
            PermissionResult::Passthrough => {
                // Tool has no opinion, continue pipeline
            }
        }

        // Step 3: DontAsk mode bypass
        if self.mode == PermissionMode::DontAsk {
            return PermissionDecision::Allow;
        }

        // Step 4: AcceptEdits mode — auto-allow file tools
        if self.mode == PermissionMode::AcceptEdits && is_file_tool {
            return PermissionDecision::Allow;
        }

        // Step 5: Allow rules
        if self.matches_rules(&self.allow_rules, tool_name, input, permission_matcher) {
            return PermissionDecision::Allow;
        }

        // Step 6: Read-only tools
        if is_read_only {
            return PermissionDecision::Allow;
        }

        // Step 7: Default → ask
        PermissionDecision::NeedsApproval {
            tool_name: tool_name.to_string(),
            description: desc,
            risk_level: RiskLevel::Low,
            suggestion,
        }
    }

    // ── Rule matching ────────────────────────────────────────────────

    /// Check if any rule in the list matches the given tool + input.
    fn matches_rules(
        &self,
        rules: &[PermissionRuleValue],
        tool_name: &str,
        _input: &serde_json::Value,
        permission_matcher: Option<&dyn Fn(&str) -> bool>,
    ) -> bool {
        for rule in rules {
            if rule.tool_name != tool_name {
                continue;
            }
            match (&rule.rule_content, permission_matcher) {
                // Rule has content and tool provides a matcher → delegate
                (Some(content), Some(matcher)) => {
                    if matcher(content) {
                        return true;
                    }
                }
                // Rule has no content → matches entire tool
                (None, _) => {
                    return true;
                }
                // Rule has content but tool provides no matcher → skip
                (Some(_), None) => {}
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Helper: build a PermissionState with the given mode and no rules.
    fn state(mode: PermissionMode) -> PermissionState {
        PermissionState::new(mode)
    }

    /// Helper: check the pipeline with simple defaults (Passthrough, not read-only, not file tool).
    fn check_simple(
        ps: &PermissionState,
        tool: &str,
        tool_check: &PermissionResult,
    ) -> PermissionDecision {
        ps.check(
            tool,
            &json!({}),
            tool_check,
            false, // is_read_only
            false, // is_file_tool
            format!("{} wants to execute", tool),
            None,  // no matcher
            None,  // no suggestion
        )
    }

    // ── Step 1: Deny rules ───────────────────────────────────────────

    #[test]
    fn deny_rule_blocks_tool() {
        let mut ps = state(PermissionMode::Default);
        ps.add_deny_rule("Bash", None);

        let decision = check_simple(&ps, "Bash", &PermissionResult::Passthrough);
        assert!(matches!(decision, PermissionDecision::Deny { .. }));
    }

    #[test]
    fn deny_rule_with_content_uses_matcher() {
        let mut ps = state(PermissionMode::Default);
        ps.add_deny_rule("Bash", Some("rm:*"));

        // With a matcher that matches "rm -rf /tmp" against "rm:*"
        let matcher = |pattern: &str| -> bool {
            if let Some(prefix) = pattern.strip_suffix(":*") {
                "rm -rf /tmp".starts_with(prefix)
            } else {
                false
            }
        };

        let decision = ps.check(
            "Bash", &json!({}), &PermissionResult::Passthrough,
            false, false, "desc".into(), Some(&matcher), None,
        );
        assert!(matches!(decision, PermissionDecision::Deny { .. }));
    }

    #[test]
    fn deny_rule_with_content_does_not_match_other_commands() {
        let mut ps = state(PermissionMode::Default);
        ps.add_deny_rule("Bash", Some("rm:*"));

        let matcher = |pattern: &str| -> bool {
            if let Some(prefix) = pattern.strip_suffix(":*") {
                "git status".starts_with(prefix) // git ≠ rm
            } else {
                false
            }
        };

        let decision = ps.check(
            "Bash", &json!({}), &PermissionResult::Passthrough,
            false, false, "desc".into(), Some(&matcher), None,
        );
        // Should NOT be denied — rule doesn't match
        assert!(!matches!(decision, PermissionDecision::Deny { .. }));
    }

    // ── Step 2: Tool self-check ──────────────────────────────────────

    #[test]
    fn tool_check_allow_is_honoured() {
        let ps = state(PermissionMode::Default);
        let decision = check_simple(&ps, "FileRead", &PermissionResult::Allow);
        assert!(matches!(decision, PermissionDecision::Allow));
    }

    #[test]
    fn tool_check_deny_is_honoured() {
        let ps = state(PermissionMode::Default);
        let deny = PermissionResult::Deny { message: "path traversal".into() };
        let decision = check_simple(&ps, "FileWrite", &deny);
        assert!(matches!(decision, PermissionDecision::Deny { .. }));
    }

    #[test]
    fn tool_check_ask_prompts_user() {
        let ps = state(PermissionMode::Default);
        let ask = PermissionResult::Ask { message: "dangerous file".into() };
        let decision = check_simple(&ps, "FileWrite", &ask);
        match decision {
            PermissionDecision::NeedsApproval { risk_level, .. } => {
                assert_eq!(risk_level, RiskLevel::Medium);
            }
            _ => panic!("expected NeedsApproval"),
        }
    }

    #[test]
    fn tool_check_ask_overridden_by_dontask() {
        let ps = state(PermissionMode::DontAsk);
        let ask = PermissionResult::Ask { message: "dangerous file".into() };
        let decision = check_simple(&ps, "FileWrite", &ask);
        assert!(matches!(decision, PermissionDecision::Allow));
    }

    #[test]
    fn tool_check_ask_overridden_by_allow_rule() {
        let mut ps = state(PermissionMode::Default);
        ps.add_allow_rule("FileWrite", None);
        let ask = PermissionResult::Ask { message: "dangerous file".into() };
        let decision = check_simple(&ps, "FileWrite", &ask);
        assert!(matches!(decision, PermissionDecision::Allow));
    }

    // ── Step 3: DontAsk mode ─────────────────────────────────────────

    #[test]
    fn dontask_mode_allows_all_passthrough() {
        let ps = state(PermissionMode::DontAsk);
        let decision = check_simple(&ps, "Bash", &PermissionResult::Passthrough);
        assert!(matches!(decision, PermissionDecision::Allow));
    }

    // ── Step 4: AcceptEdits mode ─────────────────────────────────────

    #[test]
    fn accept_edits_allows_file_tools() {
        let ps = state(PermissionMode::AcceptEdits);
        let decision = ps.check(
            "FileWrite", &json!({}), &PermissionResult::Passthrough,
            false, true, // is_file_tool = true
            "write file".into(), None, None,
        );
        assert!(matches!(decision, PermissionDecision::Allow));
    }

    #[test]
    fn accept_edits_does_not_allow_bash() {
        let ps = state(PermissionMode::AcceptEdits);
        let decision = ps.check(
            "Bash", &json!({}), &PermissionResult::Passthrough,
            false, false, // is_file_tool = false
            "run cmd".into(), None, None,
        );
        assert!(matches!(decision, PermissionDecision::NeedsApproval { .. }));
    }

    // ── Step 5: Allow rules ──────────────────────────────────────────

    #[test]
    fn allow_rule_broad_matches_all() {
        let mut ps = state(PermissionMode::Default);
        ps.add_allow_rule("Bash", None);
        let decision = check_simple(&ps, "Bash", &PermissionResult::Passthrough);
        assert!(matches!(decision, PermissionDecision::Allow));
    }

    #[test]
    fn allow_rule_with_content_matches_via_matcher() {
        let mut ps = state(PermissionMode::Default);
        ps.add_allow_rule("Bash", Some("git commit:*"));

        let matcher = |pattern: &str| -> bool {
            if let Some(prefix) = pattern.strip_suffix(":*") {
                "git commit -m 'fix'".starts_with(prefix)
            } else {
                false
            }
        };

        let decision = ps.check(
            "Bash", &json!({}), &PermissionResult::Passthrough,
            false, false, "desc".into(), Some(&matcher), None,
        );
        assert!(matches!(decision, PermissionDecision::Allow));
    }

    #[test]
    fn allow_rule_with_content_does_not_match_different_command() {
        let mut ps = state(PermissionMode::Default);
        ps.add_allow_rule("Bash", Some("git commit:*"));

        let matcher = |pattern: &str| -> bool {
            if let Some(prefix) = pattern.strip_suffix(":*") {
                "git push origin main".starts_with(prefix) // push ≠ commit
            } else {
                false
            }
        };

        let decision = ps.check(
            "Bash", &json!({}), &PermissionResult::Passthrough,
            false, false, "desc".into(), Some(&matcher), None,
        );
        // Not matched by rule, falls through to default ask
        assert!(matches!(decision, PermissionDecision::NeedsApproval { .. }));
    }

    #[test]
    fn allow_rule_for_wrong_tool_does_not_match() {
        let mut ps = state(PermissionMode::Default);
        ps.add_allow_rule("FileWrite", None);
        let decision = check_simple(&ps, "Bash", &PermissionResult::Passthrough);
        // Bash not allowed, should ask
        assert!(matches!(decision, PermissionDecision::NeedsApproval { .. }));
    }

    // ── Step 6: Read-only auto-allow ─────────────────────────────────

    #[test]
    fn read_only_tool_auto_allowed() {
        let ps = state(PermissionMode::Default);
        let decision = ps.check(
            "FileRead", &json!({}), &PermissionResult::Passthrough,
            true, false, // is_read_only = true
            "read file".into(), None, None,
        );
        assert!(matches!(decision, PermissionDecision::Allow));
    }

    // ── Step 7: Default ask ──────────────────────────────────────────

    #[test]
    fn default_mode_asks_for_write_tool() {
        let ps = state(PermissionMode::Default);
        let decision = check_simple(&ps, "Bash", &PermissionResult::Passthrough);
        match decision {
            PermissionDecision::NeedsApproval { risk_level, .. } => {
                assert_eq!(risk_level, RiskLevel::Low);
            }
            _ => panic!("expected NeedsApproval"),
        }
    }

    #[test]
    fn default_ask_carries_suggestion() {
        let ps = state(PermissionMode::Default);
        let decision = ps.check(
            "Bash", &json!({}), &PermissionResult::Passthrough,
            false, false, "desc".into(), None, Some("git commit:*".into()),
        );
        match decision {
            PermissionDecision::NeedsApproval { suggestion, .. } => {
                assert_eq!(suggestion, Some("git commit:*".into()));
            }
            _ => panic!("expected NeedsApproval"),
        }
    }

    // ── Rule management ──────────────────────────────────────────────

    #[test]
    fn duplicate_rules_are_deduplicated() {
        let mut ps = state(PermissionMode::Default);
        ps.add_allow_rule("Bash", None);
        ps.add_allow_rule("Bash", None);
        assert_eq!(ps.allow_rules.len(), 1);
    }

    #[test]
    fn different_content_rules_are_separate() {
        let mut ps = state(PermissionMode::Default);
        ps.add_allow_rule("Bash", Some("git commit:*"));
        ps.add_allow_rule("Bash", Some("git push:*"));
        assert_eq!(ps.allow_rules.len(), 2);
    }

    // ── Pipeline priority ────────────────────────────────────────────

    #[test]
    fn deny_rule_beats_allow_rule() {
        let mut ps = state(PermissionMode::Default);
        ps.add_allow_rule("Bash", None);
        ps.add_deny_rule("Bash", None);

        let decision = check_simple(&ps, "Bash", &PermissionResult::Passthrough);
        // Deny is checked first (step 1) before allow (step 5)
        assert!(matches!(decision, PermissionDecision::Deny { .. }));
    }

    #[test]
    fn tool_deny_beats_dontask_mode() {
        let ps = state(PermissionMode::DontAsk);
        let deny = PermissionResult::Deny { message: "blocked".into() };
        let decision = check_simple(&ps, "Bash", &deny);
        // Tool deny (step 2) happens before DontAsk (step 3)
        assert!(matches!(decision, PermissionDecision::Deny { .. }));
    }
}
