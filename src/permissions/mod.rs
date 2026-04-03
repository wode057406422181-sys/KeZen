use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionMode {
    /// Write operations prompt users (default)
    Default,
    /// Bypass permission checks (--yes / -y)
    DontAsk,
}

#[derive(Debug, Clone)]
pub enum PermissionCheck {
    Allow,
    NeedsApproval {
        tool_name: String,
        description: String,
    },
}

pub struct PermissionState {
    pub mode: PermissionMode,
    always_allowed: HashSet<String>,
}

impl PermissionState {
    pub fn new(mode: PermissionMode) -> Self {
        Self {
            mode,
            always_allowed: HashSet::new(),
        }
    }

    pub fn check(&self, tool_name: &str, is_read_only: bool, desc: String) -> PermissionCheck {
        if self.mode == PermissionMode::DontAsk {
            return PermissionCheck::Allow;
        }

        if is_read_only {
            return PermissionCheck::Allow;
        }

        if self.always_allowed.contains(tool_name) {
            return PermissionCheck::Allow;
        }

        PermissionCheck::NeedsApproval {
            tool_name: tool_name.to_string(),
            description: desc,
        }
    }

    pub fn always_allow(&mut self, tool_name: &str) {
        self.always_allowed.insert(tool_name.to_string());
    }
}
