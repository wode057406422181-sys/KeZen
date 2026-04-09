use crate::permissions::PermissionMode;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Cluster top-level configuration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ClusterConfig {
    #[serde(default)]
    pub cluster: ClusterContext,
    #[serde(default)]
    pub defaults: DefaultsConfig,
    #[serde(default)]
    pub agents: Vec<AgentConfig>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ClusterContext {
    pub name: Option<String>,
    pub namespace: Option<String>,
    pub work_dir: Option<PathBuf>,
    #[serde(default)]
    pub permissions: Option<PermissionConfig>,
    #[serde(default)]
    pub mcp_servers: HashMap<String, McpServerConfig>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DefaultsConfig {
    pub model: Option<String>,
    pub context_window: Option<usize>,
    pub max_tokens: Option<usize>,
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
}

/// Agent Kind definition
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AgentKind {
    Gateway,
    Pod,
    Worker,
}

/// Agent Configuration node. Can be Gateway, Pod, or Worker.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentConfig {
    pub kind: Option<AgentKind>,
    pub name: Option<String>,
    pub model: Option<String>,

    /// Scope logic (e.g. "global", "testing")
    pub scope: Option<String>,

    /// Work dir for this agent
    pub work_dir: Option<PathBuf>,

    /// Memory: either inline string or a file path
    pub memory: Option<String>,

    #[serde(default)]
    pub skills: Vec<String>,

    pub mcp_servers: Option<Vec<String>>,

    #[serde(default)]
    pub tools: Vec<String>,

    pub max_concurrent_tasks: Option<usize>,

    #[serde(default)]
    pub permissions: Option<PermissionConfig>,

    #[serde(default)]
    pub resource_limits: Option<ResourceLimitsConfig>,

    #[serde(default)]
    pub access_points: Vec<AccessPointConfig>,

    pub master: Option<Box<AgentConfig>>,

    #[serde(default)]
    pub workers: Vec<AgentConfig>,
}

/// Permission configuration, supporting patch semantics (all optional)
/// TODO: impl From<PermissionConfig> for crate::permissions::PermissionState to bridge configuration to runtime state
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PermissionConfig {
    pub mode: Option<PermissionMode>,
    pub auto_approve: Option<Vec<String>>,
    pub require_approval: Option<Vec<String>>,
    pub allow_rules: Option<Vec<String>>,
    pub deny_rules: Option<Vec<String>>,
    pub allow_cross_workdir: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResourceLimitsConfig {
    pub max_tokens_per_turn: Option<usize>,
    pub max_tool_iterations: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum AccessPointConfig {
    Tui {
        can_approve: Option<bool>,
    },
    Repl {
        can_approve: Option<bool>,
    },
    Grpc {
        listen: String,
        auth: Option<String>,
        can_approve: Option<bool>,
    },
}

/// Utility function to parse the kezen.toml from a given path
pub async fn load_cluster_config(path: &Path) -> anyhow::Result<ClusterConfig> {
    let content = tokio::fs::read_to_string(path).await?;
    let config: ClusterConfig = toml::from_str(&content)
        .with_context(|| format!("Failed to parse cluster config: {}", path.display()))?;
    Ok(config)
}

/// Returns true if the memory value should be treated as a file path.
pub fn is_memory_file_path(memory: &str) -> bool {
    memory.ends_with(".md") || memory.ends_with(".yaml") || memory.ends_with(".txt")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_file_path_check() {
        assert!(is_memory_file_path("system_prompt.md"));
        assert!(is_memory_file_path("/abs/system_prompt.yaml"));
        assert!(is_memory_file_path("../relative/system_prompt.txt"));
        assert!(!is_memory_file_path("You are a helpful assistant."));
    }

    #[test]
    fn test_parse_invalid_fields() {
        let toml_invalid_kind = r#"
        [[agents]]
        kind = "Leader"
        "#;
        let res: Result<ClusterConfig, _> = toml::from_str(toml_invalid_kind);
        assert!(res.is_err()); // 'Leader' is not a valid AgentKind

        let toml_invalid_mode = r#"
        [cluster.permissions]
        mode = "yolo"
        "#;
        let res2: Result<ClusterConfig, _> = toml::from_str(toml_invalid_mode);
        assert!(res2.is_err()); // 'yolo' is not a valid permission mode
    }

    #[test]
    fn test_full_implementation_plan_toml() {
        let toml_str = r#"
[cluster]
name      = "full-stack-dev"
namespace = "default"
work_dir  = "/workspace"

[defaults]
model           = "claude-3-5-sonnet-latest"
context_window  = 200_000
max_tokens      = 16_384
timeout_seconds = 300

[cluster.permissions]
mode             = "default"
auto_approve     = ["FileRead", "Grep", "Glob"]
require_approval = ["FileWrite", "FileEdit", "Bash"]
allow_rules      = []
deny_rules       = []

[[agents]]
kind  = "Gateway"
name  = "gateway"

  [[agents.access_points]]
  type        = "tui"
  can_approve = true

  [[agents.access_points]]
  type        = "repl"
  can_approve = true

  [[agents.access_points]]
  type        = "grpc"
  listen      = "127.0.0.1:50052"
  auth        = "local_socket"
  can_approve = true

  [[agents.workers]]
  kind  = "Pod"
  name  = "orchestrator"
  scope = "global"

    [agents.workers.master]
    name  = "architect"
    model = "claude-3-5-sonnet-latest"
    memory = """
You are the chief architect.
You break down requirements into tasks.
"""
    skills               = ["code-review", "architecture"]
    mcp_servers          = ["filesystem"]
    tools                = ["FileRead", "Grep", "Glob"]
    max_concurrent_tasks = 5

    [[agents.workers.workers]]
    kind   = "Worker"
    name   = "coder"
    work_dir = "/workspace/src"
    memory   = "agents/coder.md"
    skills   = ["rust-coding"]
    mcp_servers = []
    tools    = ["FileRead", "FileWrite"]

      [agents.workers.workers.permissions]
      mode         = "accept_edits"
      require_approval = ["Bash"]
      allow_rules  = ["Bash:git commit:*"]
      deny_rules   = ["Bash:rm -rf:*"]

      [agents.workers.workers.resource_limits]
      max_tokens_per_turn = 8192
      max_tool_iterations = 15

      [[agents.workers.workers.access_points]]
      type        = "grpc"
      listen      = "127.0.0.1:50053"
      can_approve = false

    [[agents.workers.workers]]
    kind     = "Pod"
    name     = "test-crew"
    scope    = "testing"
    work_dir = "/workspace"

      [agents.workers.workers.master]
      name   = "test-lead"
      model  = "claude-3-haiku-latest"
      memory = "You lead the test team."
      tools  = ["FileRead", "Bash", "Grep"]

      [[agents.workers.workers.workers]]
      kind     = "Worker"
      name     = "unit-tester"
      model    = "claude-3-haiku-latest"
      work_dir = "/workspace"
      memory   = "agents/unit-tester.md"
      tools    = ["FileRead", "FileWrite", "Bash", "Grep"]

        [agents.workers.workers.workers.permissions]
        mode        = "accept_edits"
        allow_rules = ["Bash:cargo test:*", "Bash:cargo build:*"]
        deny_rules  = ["Bash:rm:*"]
        "#;

        let parsed: ClusterConfig = toml::from_str(toml_str).expect("Should parse full E2E TOML");

        // Assertions
        assert_eq!(parsed.cluster.name.unwrap(), "full-stack-dev");
        assert_eq!(
            parsed.cluster.permissions.clone().unwrap().mode.unwrap(),
            PermissionMode::Default
        );

        assert_eq!(parsed.agents.len(), 1);
        let gateway = &parsed.agents[0];
        assert_eq!(gateway.name, Some("gateway".to_string()));
        assert_eq!(gateway.kind, Some(AgentKind::Gateway));

        // APs
        assert_eq!(gateway.access_points.len(), 3);
        match &gateway.access_points[1] {
            AccessPointConfig::Repl { can_approve } => assert_eq!(*can_approve, Some(true)),
            _ => panic!("Expected REPL AP"),
        }

        // Pod
        let pod = &gateway.workers[0];
        assert_eq!(pod.kind, Some(AgentKind::Pod));
        let master = pod.master.as_ref().unwrap();
        assert_eq!(master.name, Some("architect".to_string()));
        assert!(
            master
                .memory
                .as_ref()
                .unwrap()
                .contains("You break down requirements")
        );
        assert_eq!(master.skills.len(), 2);
        assert_eq!(master.max_concurrent_tasks, Some(5));

        // Worker: coder
        let coder = &pod.workers[0];
        assert_eq!(coder.name, Some("coder".to_string()));
        assert_eq!(
            coder.permissions.as_ref().unwrap().mode,
            Some(PermissionMode::AcceptEdits)
        );

        // Nested Pod: test-crew
        let test_crew = &pod.workers[1];
        assert_eq!(test_crew.name, Some("test-crew".to_string()));
        assert_eq!(test_crew.kind, Some(AgentKind::Pod));

        let lead = test_crew.master.as_ref().unwrap();
        assert_eq!(lead.name, Some("test-lead".to_string()));

        let tester = &test_crew.workers[0];
        assert_eq!(tester.name, Some("unit-tester".to_string()));
        assert_eq!(
            tester
                .permissions
                .as_ref()
                .unwrap()
                .allow_rules
                .as_ref()
                .unwrap()
                .len(),
            2
        );
    }
}
