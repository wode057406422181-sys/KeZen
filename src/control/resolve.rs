use crate::control::topology::{AgentConfig, ClusterConfig, PermissionConfig};
use std::env;
use std::path::{Path, PathBuf};

/// A fully resolved agent node in the cluster topology.
///
/// `config` contains the agent's own declared fields (name, kind, scope, etc.)
/// with `workers` and `master` cleared to avoid confusion — use the resolved
/// `self.workers` and `self.master` fields instead.
#[derive(Debug, Clone)]
pub struct ResolvedAgent {
    pub config: AgentConfig,
    pub resolved_model: Option<String>,
    pub resolved_work_dir: PathBuf,
    pub resolved_permissions: PermissionConfig,
    pub master: Option<Box<ResolvedAgent>>,
    pub workers: Vec<ResolvedAgent>,
}

/// Resolves the work_dir for an agent based on the 4-level inheritance rule:
/// 1. Agent's own explicitly set work_dir
/// 2. Parent's resolved work_dir
/// 3. Cluster scope work_dir
/// 4. Current working directory fallback
///
/// NOTE: Returned path may be relative. The caller is responsible for resolving
/// it against the appropriate base (e.g. the directory containing kezen.toml).
pub fn resolve_work_dir(
    agent_work_dir: Option<&Path>,
    parent_work_dir: Option<&Path>,
    cluster_work_dir: Option<&Path>,
) -> PathBuf {
    if let Some(wd) = agent_work_dir {
        return wd.to_path_buf();
    }
    if let Some(pwd) = parent_work_dir {
        return pwd.to_path_buf();
    }
    if let Some(cwd) = cluster_work_dir {
        return cwd.to_path_buf();
    }
    env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Resolves the effective permission configuration by merging base (parent) and patch (child).
/// - `mode`, `auto_approve`, `require_approval`, `allow_cross_workdir`: patch replaces base.
/// - `allow_rules`, `deny_rules`: patch appends to base.
pub fn resolve_permissions(
    base: Option<&PermissionConfig>,
    patch: Option<&PermissionConfig>,
) -> PermissionConfig {
    let mut resolved = base.cloned().unwrap_or_default();

    if let Some(p) = patch {
        if let Some(ref mode) = p.mode {
            resolved.mode = Some(*mode);
        }
        if let Some(ref auth) = p.auto_approve {
            resolved.auto_approve = Some(auth.clone());
        }
        if let Some(ref req) = p.require_approval {
            resolved.require_approval = Some(req.clone());
        }
        if let Some(ref allow) = p.allow_rules {
            let mut new_allows = resolved.allow_rules.unwrap_or_default();
            new_allows.extend(allow.iter().cloned());
            resolved.allow_rules = Some(new_allows);
        }
        if let Some(ref deny) = p.deny_rules {
            let mut new_denies = resolved.deny_rules.unwrap_or_default();
            new_denies.extend(deny.iter().cloned());
            resolved.deny_rules = Some(new_denies);
        }
        if let Some(cross) = p.allow_cross_workdir {
            resolved.allow_cross_workdir = Some(cross);
        }
    }

    resolved
}

/// Recursively resolves an entire cluster into a tree of fully resolved agents.
///
/// Each agent inherits work_dir, permissions, and model from its parent chain
/// and the cluster-level defaults.
pub fn resolve_tree(cluster: &ClusterConfig) -> Vec<ResolvedAgent> {
    let cluster_work_dir = cluster.cluster.work_dir.as_deref();
    let cluster_permissions = cluster.cluster.permissions.as_ref();
    let default_model = cluster.defaults.model.as_deref();

    cluster
        .agents
        .iter()
        .map(|agent| {
            resolve_agent(
                agent,
                cluster_work_dir,
                cluster_permissions,
                default_model,
                cluster,
            )
        })
        .collect()
}

/// Resolves a single agent node within the cluster tree.
///
/// For top-level agents (called from `resolve_tree`), `parent_work_dir` is the
/// cluster-level work_dir — this intentionally overlaps with the third fallback
/// tier in `resolve_work_dir`, since top-level agents have no real parent.
///
/// For nested agents (workers, master), `parent_work_dir` is the already-resolved
/// work_dir of the enclosing agent, providing proper 4-level inheritance.
fn resolve_agent(
    agent: &AgentConfig,
    parent_work_dir: Option<&Path>,
    parent_permissions: Option<&PermissionConfig>,
    default_model: Option<&str>,
    cluster: &ClusterConfig,
) -> ResolvedAgent {
    let resolved_work_dir = resolve_work_dir(
        agent.work_dir.as_deref(),
        parent_work_dir,
        cluster.cluster.work_dir.as_deref(),
    );

    let resolved_permissions = resolve_permissions(parent_permissions, agent.permissions.as_ref());

    let resolved_model = agent
        .model
        .clone()
        .or_else(|| default_model.map(String::from));

    let master = agent.master.as_ref().map(|m| {
        Box::new(resolve_agent(
            m,
            Some(&resolved_work_dir),
            Some(&resolved_permissions),
            default_model,
            cluster,
        ))
    });

    let workers = agent
        .workers
        .iter()
        .map(|w| {
            resolve_agent(
                w,
                Some(&resolved_work_dir),
                Some(&resolved_permissions),
                default_model,
                cluster,
            )
        })
        .collect();

    // Clone config but clear the tree fields to avoid confusion with the
    // resolved `self.master` and `self.workers`.
    let mut config = agent.clone();
    config.master = None;
    config.workers = Vec::new();

    ResolvedAgent {
        config,
        resolved_model,
        resolved_work_dir,
        resolved_permissions,
        master,
        workers,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::topology::{AgentKind, ClusterContext, DefaultsConfig};
    use crate::permissions::PermissionMode;

    #[test]
    fn test_resolve_work_dir_self() {
        let wd = resolve_work_dir(
            Some(Path::new("/agent_explicit")),
            Some(Path::new("/parent_dir")),
            Some(Path::new("/cluster_dir")),
        );
        assert_eq!(wd, PathBuf::from("/agent_explicit"));
    }

    #[test]
    fn test_resolve_work_dir_parent() {
        let wd = resolve_work_dir(
            None,
            Some(Path::new("/parent_dir")),
            Some(Path::new("/cluster_dir")),
        );
        assert_eq!(wd, PathBuf::from("/parent_dir"));
    }

    #[test]
    fn test_resolve_work_dir_cluster() {
        let wd = resolve_work_dir(None, None, Some(Path::new("/cluster_dir")));
        assert_eq!(wd, PathBuf::from("/cluster_dir"));
    }

    #[test]
    fn test_resolve_work_dir_fallback() {
        let wd = resolve_work_dir(None, None, None);
        assert_eq!(wd, env::current_dir().unwrap());
    }

    #[test]
    fn test_resolve_permissions_override_mode() {
        let base = PermissionConfig {
            mode: Some(PermissionMode::Default),
            auto_approve: Some(vec!["FileRead".to_string()]),
            ..Default::default()
        };
        let patch = PermissionConfig {
            mode: Some(PermissionMode::DontAsk),
            ..Default::default()
        };
        let resolved = resolve_permissions(Some(&base), Some(&patch));
        assert_eq!(resolved.mode.unwrap(), PermissionMode::DontAsk);
        assert_eq!(resolved.auto_approve.unwrap(), vec!["FileRead".to_string()]);
    }

    #[test]
    fn test_resolve_permissions_replace_list() {
        let base = PermissionConfig {
            require_approval: Some(vec!["A".into(), "B".into()]),
            ..Default::default()
        };
        let patch = PermissionConfig {
            require_approval: Some(vec!["C".into()]),
            ..Default::default()
        };
        let resolved = resolve_permissions(Some(&base), Some(&patch));
        assert_eq!(resolved.require_approval.unwrap(), vec!["C".to_string()]);
    }

    #[test]
    fn test_resolve_permissions_append_rules() {
        let base = PermissionConfig {
            allow_rules: Some(vec!["rule1".into()]),
            deny_rules: Some(vec!["deny1".into()]),
            ..Default::default()
        };
        let patch = PermissionConfig {
            allow_rules: Some(vec!["rule2".into()]),
            deny_rules: Some(vec!["deny2".into()]),
            ..Default::default()
        };
        let resolved = resolve_permissions(Some(&base), Some(&patch));
        assert_eq!(
            resolved.allow_rules.unwrap(),
            vec!["rule1".to_string(), "rule2".to_string()]
        );
        assert_eq!(
            resolved.deny_rules.unwrap(),
            vec!["deny1".to_string(), "deny2".to_string()]
        );
    }

    #[test]
    fn test_resolve_permissions_no_patch() {
        let base = PermissionConfig {
            mode: Some(PermissionMode::AcceptEdits),
            ..Default::default()
        };
        let resolved = resolve_permissions(Some(&base), None);
        assert_eq!(resolved.mode.unwrap(), PermissionMode::AcceptEdits);
    }

    #[test]
    fn test_resolve_tree_integration() {
        let mut cluster = ClusterConfig::default();
        cluster.cluster = ClusterContext {
            work_dir: Some(PathBuf::from("/cluster")),
            permissions: Some(PermissionConfig {
                mode: Some(PermissionMode::Default),
                allow_rules: Some(vec!["read".into()]),
                ..Default::default()
            }),
            ..Default::default()
        };
        cluster.defaults = DefaultsConfig {
            model: Some("claude-sonnet".into()),
            ..Default::default()
        };

        let mut gateway = AgentConfig::default();
        gateway.kind = Some(AgentKind::Gateway);
        gateway.name = Some("gateway".into());

        let mut pod = AgentConfig::default();
        pod.kind = Some(AgentKind::Pod);
        pod.name = Some("pod".into());
        pod.work_dir = Some(PathBuf::from("/pod"));
        pod.permissions = Some(PermissionConfig {
            mode: Some(PermissionMode::DontAsk),
            allow_rules: Some(vec!["write".into()]),
            ..Default::default()
        });

        // master with explicit model
        let mut master = AgentConfig::default();
        master.name = Some("architect".into());
        master.model = Some("claude-opus".into());
        pod.master = Some(Box::new(master));

        // worker with no model → should fallback to defaults
        let mut worker = AgentConfig::default();
        worker.kind = Some(AgentKind::Worker);
        worker.name = Some("worker".into());

        pod.workers.push(worker);
        gateway.workers.push(pod);
        cluster.agents.push(gateway);

        let resolved_tree = resolve_tree(&cluster);
        assert_eq!(resolved_tree.len(), 1);

        // Gateway: inherits cluster work_dir, no explicit model → defaults
        let gw = &resolved_tree[0];
        assert_eq!(gw.resolved_work_dir, PathBuf::from("/cluster"));
        assert_eq!(gw.resolved_permissions.mode, Some(PermissionMode::Default));
        assert_eq!(gw.resolved_model.as_deref(), Some("claude-sonnet"));

        // Gateway's config.workers should be cleared
        assert!(gw.config.workers.is_empty());
        // But the resolved tree has the workers
        assert_eq!(gw.workers.len(), 1);

        // Pod: explicit work_dir and permissions
        let pod_r = &gw.workers[0];
        assert_eq!(pod_r.resolved_work_dir, PathBuf::from("/pod"));
        assert_eq!(
            pod_r.resolved_permissions.mode,
            Some(PermissionMode::DontAsk)
        );
        let allow = pod_r.resolved_permissions.allow_rules.as_ref().unwrap();
        assert_eq!(allow.len(), 2);
        assert!(allow.contains(&"read".into()));
        assert!(allow.contains(&"write".into()));
        // Pod config.master should be cleared
        assert!(pod_r.config.master.is_none());
        // But resolved master is present
        assert!(pod_r.master.is_some());

        // Master: explicit model overrides defaults
        let master_r = pod_r.master.as_ref().unwrap();
        assert_eq!(master_r.resolved_model.as_deref(), Some("claude-opus"));
        // Master inherits pod's resolved work_dir and permissions
        assert_eq!(master_r.resolved_work_dir, PathBuf::from("/pod"));
        assert_eq!(
            master_r.resolved_permissions.mode,
            Some(PermissionMode::DontAsk)
        );

        // Worker: inherits pod's work_dir/permissions, defaults model
        let worker_r = &pod_r.workers[0];
        assert_eq!(worker_r.resolved_work_dir, PathBuf::from("/pod"));
        assert_eq!(
            worker_r.resolved_permissions.mode,
            Some(PermissionMode::DontAsk)
        );
        assert_eq!(worker_r.resolved_model.as_deref(), Some("claude-sonnet"));
    }
}
