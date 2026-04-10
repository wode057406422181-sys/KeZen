use async_trait::async_trait;
use std::path::PathBuf;
use tokio::sync::{RwLock, broadcast, mpsc};

use super::access_point::AccessPoint;
use super::agent::{AgentId, AgentNode, AgentStatus, AgentTask, AgentTaskResult};
use super::bus;
use super::worker::LlmWorkerNode;
use crate::config::AppConfig;
use crate::engine::events::{EngineEvent, UserAction};
use crate::permissions::PermissionMode;

/// 子节点的运行时句柄。
///
/// 持有与子 Agent 通信所需的 `ChannelPair` 和 `AgentNode` trait object。
/// Pod 通过 ChildHandle 向子节点分发任务并接收事件。
pub struct ChildHandle {
    /// 子节点的 AgentNode 实现。
    pub node: Box<dyn AgentNode>,
}

/// PodNode — 容器节点，持有 Master Engine + 子节点集合。
///
/// Pod 的 Engine 作为 "Master"，负责：
///   1. 接收上级任务
///   2. 拆解为子任务（通过 Master Engine 的 LLM 推理）
///   3. 分发到 children（向子节点的 mpsc 发送 UserAction::SendMessage）
///   4. 聚合子节点结果（监听子节点的 EngineEvent 事件流）
///   5. 合并结果并向上报告
///
/// 一期简化实现：Pod 将任务直接下发到第一个 Worker，
/// Master Engine 的调度逻辑在后续迭代中实现。
pub struct PodNode {
    id: AgentId,
    access_points: Vec<AccessPoint>,
    status: RwLock<AgentStatus>,

    /// 子节点句柄列表。
    children_handles: RwLock<Vec<ChildHandle>>,
    /// 子节点 ID 列表（用于 AgentNode::children() 返回）。
    children_ids: Vec<AgentId>,

    /// Master Engine 的配置和通信通道。
    config: AppConfig,
    work_dir: PathBuf,
    permission_mode: PermissionMode,

    /// Master Engine 的 action channel（用于向 Master 发送指令）。
    master_action_tx: mpsc::Sender<UserAction>,
    /// Master Engine 的 action_rx，init() 时取出。
    master_action_rx: RwLock<Option<mpsc::Receiver<UserAction>>>,
    /// Master Engine 的 event broadcast（用于订阅 Master 事件）。
    master_event_tx: broadcast::Sender<EngineEvent>,
    /// Master Engine 的 tokio task handle。
    master_handle: RwLock<Option<tokio::task::JoinHandle<()>>>,
}

impl PodNode {
    /// 构造一个新的 PodNode。
    ///
    /// 传入子节点的 ChildHandle 列表。Master Engine 在 `init()` 时构建。
    pub fn new(
        id: AgentId,
        access_points: Vec<AccessPoint>,
        children_handles: Vec<ChildHandle>,
        config: AppConfig,
        work_dir: PathBuf,
        permission_mode: PermissionMode,
    ) -> Self {
        let children_ids: Vec<AgentId> = children_handles
            .iter()
            .map(|ch| ch.node.id().clone())
            .collect();

        let master_channels = bus::create_default_channel_pair();
        let master_action_tx = master_channels.action_tx;
        let master_action_rx = master_channels.action_rx;
        let master_event_tx = master_channels.event_tx;

        Self {
            id,
            access_points,
            status: RwLock::new(AgentStatus::Created),
            children_handles: RwLock::new(children_handles),
            children_ids,
            config,
            work_dir,
            permission_mode,
            master_action_tx,
            master_action_rx: RwLock::new(master_action_rx),
            master_event_tx,
            master_handle: RwLock::new(None),
        }
    }

    /// 获取 Master Engine 的事件订阅 receiver。
    pub fn subscribe_master_events(&self) -> broadcast::Receiver<EngineEvent> {
        self.master_event_tx.subscribe()
    }
}

#[async_trait]
impl AgentNode for PodNode {
    fn id(&self) -> &AgentId {
        &self.id
    }

    async fn status(&self) -> AgentStatus {
        *self.status.read().await
    }

    fn access_points(&self) -> &[AccessPoint] {
        &self.access_points
    }

    async fn init(&self) -> anyhow::Result<()> {
        tracing::info!(agent = %self.id, children = self.children_ids.len(), "Pod node initializing");

        // Initialize all child nodes first.
        let children = self.children_handles.read().await;
        for child in children.iter() {
            child.node.init().await.map_err(|e| {
                anyhow::anyhow!(
                    "Failed to init child {} of pod {}: {}",
                    child.node.id(),
                    self.id,
                    e
                )
            })?;
        }
        drop(children);

        // Initialize the Master Engine.
        let action_rx = self.master_action_rx.write().await.take().ok_or_else(|| {
            anyhow::anyhow!(
                "Pod {} already initialized (master_action_rx taken)",
                self.id
            )
        })?;

        let registry =
            crate::tools::registry::create_default_registry(&self.config, self.work_dir.clone());

        let engine = crate::engine::KezenEngine::new(
            self.config.clone(),
            action_rx,
            self.master_event_tx.clone(),
            registry,
            self.permission_mode,
            self.work_dir.clone(),
        )
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "Failed to initialize Master KezenEngine for pod {}: {}",
                self.id,
                e
            )
        })?;

        let agent_id = self.id.clone();
        let handle = tokio::spawn(async move {
            tracing::info!(agent = %agent_id, "Pod master engine task started");
            engine.run().await;
            tracing::info!(agent = %agent_id, "Pod master engine task finished");
        });

        // Store the master channel handle and task handle.
        // Note: Similar to LlmWorkerNode, the action_tx/event_tx are created
        // fresh here and need to replace the initial placeholders.
        // For the trait interface, we use these internally.
        let mut master_handle = self.master_handle.write().await;
        *master_handle = Some(handle);

        let mut status = self.status.write().await;
        *status = AgentStatus::Ready;
        tracing::info!(agent = %self.id, "Pod node ready");
        Ok(())
    }

    async fn assign(&self, task: AgentTask) -> anyhow::Result<AgentTaskResult> {
        tracing::info!(
            agent = %self.id,
            task_id = %task.task_id,
            children = self.children_ids.len(),
            "Pod received task — routing to first child (simplified one-shot)"
        );

        {
            let mut status = self.status.write().await;
            *status = AgentStatus::Running;
        }

        // One-shot simplified routing: delegate to the first child.
        // Full Master-driven task decomposition is a future iteration.
        let children = self.children_handles.read().await;
        let result = if let Some(first_child) = children.first() {
            // Forward the task to the first child's AgentNode::assign().
            first_child.node.assign(task.clone()).await?
        } else {
            AgentTaskResult {
                task_id: task.task_id,
                success: false,
                output: "Pod has no children to route task to".to_string(),
                data: None,
            }
        };

        {
            let mut status = self.status.write().await;
            *status = AgentStatus::Ready;
        }

        Ok(result)
    }

    async fn suspend(&self, reason: &str) -> anyhow::Result<()> {
        tracing::info!(agent = %self.id, reason = %reason, "Pod suspending");

        // Suspend all children first.
        let children = self.children_handles.read().await;
        for child in children.iter() {
            child.node.suspend(reason).await?;
        }

        let mut status = self.status.write().await;
        *status = AgentStatus::Suspended;
        Ok(())
    }

    async fn resume(&self) -> anyhow::Result<()> {
        tracing::info!(agent = %self.id, "Pod resuming");

        let children = self.children_handles.read().await;
        for child in children.iter() {
            child.node.resume().await?;
        }

        let mut status = self.status.write().await;
        *status = AgentStatus::Ready;
        Ok(())
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        tracing::info!(agent = %self.id, "Pod shutting down");

        // Shutdown all children.
        let children = self.children_handles.read().await;
        for child in children.iter() {
            child.node.shutdown().await?;
        }
        drop(children);

        // Abort master engine task.
        let mut handle = self.master_handle.write().await;
        if let Some(h) = handle.take() {
            h.abort();
            let _ = h.await;
        }

        let mut status = self.status.write().await;
        *status = AgentStatus::Stopped;
        Ok(())
    }

    fn children(&self) -> Vec<AgentId> {
        self.children_ids.clone()
    }

    fn is_gateway(&self) -> bool {
        false
    }

    fn into_any(self: Box<Self>) -> Box<dyn std::any::Any> {
        self
    }

    fn action_sender(
        &self,
    ) -> Option<tokio::sync::mpsc::Sender<crate::engine::events::UserAction>> {
        Some(self.master_action_tx.clone())
    }

    fn subscribe_events(
        &self,
    ) -> Option<tokio::sync::broadcast::Receiver<crate::engine::events::EngineEvent>> {
        Some(self.master_event_tx.subscribe())
    }
}

/// 从 `ClusterConfig` 递归构建 `AgentNode` 树。
///
/// 遍历配置中的 `[[agents]]`，根据 `kind` 递归构建：
/// - `Gateway` → `GatewayNode`
/// - `Worker` → `LlmWorkerNode`
/// - `Pod` → `PodNode`（递归构建 `workers` + `master`）
///
/// 返回根节点（通常是 Gateway）。如果配置中有多个顶层 agent，
/// 只使用第一个作为根节点。
pub fn build_agent_tree(
    cluster: &crate::control::topology::ClusterConfig,
    base_config: &AppConfig,
    permission_mode: PermissionMode,
) -> anyhow::Result<Box<dyn AgentNode>> {
    let namespace = cluster.cluster.namespace.as_deref().unwrap_or("default");
    let cluster_work_dir = cluster
        .cluster
        .work_dir
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    let root_agent = cluster
        .agents
        .first()
        .ok_or_else(|| anyhow::anyhow!("ClusterConfig has no agents defined"))?;

    build_agent_node(
        root_agent,
        namespace,
        &cluster_work_dir,
        base_config,
        cluster,
        permission_mode,
    )
}

/// 递归构建单个 AgentNode。
pub fn build_agent_node(
    agent_config: &crate::control::topology::AgentConfig,
    namespace: &str,
    parent_work_dir: &std::path::Path,
    base_config: &AppConfig,
    cluster: &crate::control::topology::ClusterConfig,
    permission_mode: PermissionMode,
) -> anyhow::Result<Box<dyn AgentNode>> {
    use crate::control::topology::AgentKind;

    let kind = agent_config.kind.as_ref();
    let name = agent_config.name.as_deref().unwrap_or("unnamed");

    match kind {
        Some(AgentKind::Gateway) => {
            let mut gw = super::gateway::GatewayNode::from_config(agent_config, Some(namespace))?;

            let work_dir = agent_config
                .work_dir
                .clone()
                .unwrap_or_else(|| parent_work_dir.to_path_buf());

            // Recursively build child nodes for the Gateway (same as Pod).
            let mut child_handles = Vec::new();
            for worker_config in &agent_config.workers {
                let child_node = build_agent_node(
                    worker_config,
                    namespace,
                    &work_dir,
                    base_config,
                    cluster,
                    permission_mode,
                )?;
                child_handles.push(ChildHandle { node: child_node });
            }

            gw.set_children(child_handles);
            Ok(Box::new(gw))
        }
        Some(AgentKind::Worker) | None => {
            // Default to Worker if kind is not specified.
            let work_dir = agent_config
                .work_dir
                .clone()
                .unwrap_or_else(|| parent_work_dir.to_path_buf());

            let mut agent_app_config = base_config.clone();

            // Resolve model profile
            let mut model_str = None;
            if let Some(m) = &agent_config.model {
                model_str = Some(m.clone());
            } else if let Some(m) = &cluster.defaults.model {
                model_str = Some(m.clone());
            }

            if let Some(ref m) = model_str {
                if let Some(profile) = cluster.models.get(m).or_else(|| base_config.models.get(m)) {
                    agent_app_config.provider = profile.provider;
                    agent_app_config.model = Some(profile.model.clone());
                    agent_app_config.max_tokens = Some(profile.max_tokens);
                    if let Some(ref key) = profile.api_key {
                        agent_app_config.api_key = Some(key.clone());
                    }
                    if let Some(ref url) = profile.api_url {
                        agent_app_config.api_url = Some(url.clone());
                    }
                    if let Some(cw) = profile.context_window {
                        agent_app_config.context_window = Some(cw);
                    }
                    if let Some(ref ua) = profile.user_agent {
                        agent_app_config.user_agent = Some(ua.clone());
                    }
                } else {
                    agent_app_config.model = Some(m.clone());
                }
            }

            let id = AgentId(format!("{}/{}", namespace, name));

            // Convert access points from config.
            let access_points = convert_access_points(&agent_config.access_points)?;

            let worker = LlmWorkerNode::new(
                id,
                access_points,
                agent_app_config,
                work_dir,
                permission_mode,
            );
            Ok(Box::new(worker))
        }
        Some(AgentKind::Pod) => {
            let work_dir = agent_config
                .work_dir
                .clone()
                .unwrap_or_else(|| parent_work_dir.to_path_buf());

            let mut agent_app_config = base_config.clone();

            // Resolve master-level model profile if present
            let mut model_str = None;
            if let Some(ref master) = agent_config.master {
                if let Some(m) = &master.model {
                    model_str = Some(m.clone());
                }
            }
            if model_str.is_none() {
                if let Some(m) = &cluster.defaults.model {
                    model_str = Some(m.clone());
                }
            }

            if let Some(ref m) = model_str {
                if let Some(profile) = cluster.models.get(m).or_else(|| base_config.models.get(m)) {
                    agent_app_config.provider = profile.provider;
                    agent_app_config.model = Some(profile.model.clone());
                    agent_app_config.max_tokens = Some(profile.max_tokens);
                    if let Some(ref key) = profile.api_key {
                        agent_app_config.api_key = Some(key.clone());
                    }
                    if let Some(ref url) = profile.api_url {
                        agent_app_config.api_url = Some(url.clone());
                    }
                    if let Some(cw) = profile.context_window {
                        agent_app_config.context_window = Some(cw);
                    }
                    if let Some(ref ua) = profile.user_agent {
                        agent_app_config.user_agent = Some(ua.clone());
                    }
                } else {
                    agent_app_config.model = Some(m.clone());
                }
            }

            // Recursively build child nodes with channel pairs.
            let mut child_handles = Vec::new();
            for worker_config in &agent_config.workers {
                let child_node = build_agent_node(
                    worker_config,
                    namespace,
                    &work_dir,
                    base_config,
                    cluster,
                    permission_mode,
                )?;

                child_handles.push(ChildHandle { node: child_node });
            }

            let id = AgentId(format!("{}/{}", namespace, name));
            let access_points = convert_access_points(&agent_config.access_points)?;

            let pod = PodNode::new(
                id,
                access_points,
                child_handles,
                agent_app_config,
                work_dir,
                permission_mode,
            );
            Ok(Box::new(pod))
        }
    }
}

/// 将 TOML AccessPointConfig 转换为运行时 AccessPoint。
fn convert_access_points(
    configs: &[crate::control::topology::AccessPointConfig],
) -> anyhow::Result<Vec<AccessPoint>> {
    let mut aps = Vec::new();
    for ap_config in configs {
        match ap_config {
            crate::control::topology::AccessPointConfig::Tui { can_approve } => {
                aps.push(AccessPoint::Tui {
                    can_approve: can_approve.unwrap_or(true),
                });
            }
            crate::control::topology::AccessPointConfig::Repl { can_approve } => {
                aps.push(AccessPoint::Repl {
                    can_approve: can_approve.unwrap_or(true),
                });
            }
            crate::control::topology::AccessPointConfig::Grpc {
                listen,
                can_approve,
                ..
            } => {
                let addr = listen.parse().map_err(|e| {
                    anyhow::anyhow!("Invalid gRPC listen address '{}': {}", listen, e)
                })?;
                aps.push(AccessPoint::Grpc {
                    addr,
                    can_approve: can_approve.unwrap_or(false),
                });
            }
        }
    }
    Ok(aps)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_pod_config() -> AppConfig {
        AppConfig {
            model: Some("test-model".to_string()),
            api_key: Some("test-key".to_string()),
            ..AppConfig::default()
        }
    }

    #[test]
    fn pod_is_not_gateway() {
        let pod = PodNode::new(
            AgentId::from("default/orchestrator"),
            vec![],
            vec![],
            make_pod_config(),
            PathBuf::from("/tmp/test"),
            PermissionMode::DontAsk,
        );
        assert!(!pod.is_gateway());
    }

    #[test]
    fn pod_children_ids() {
        let child = LlmWorkerNode::new(
            AgentId::from("default/coder"),
            vec![],
            make_pod_config(),
            PathBuf::from("/tmp/test"),
            PermissionMode::DontAsk,
        );

        let handle = ChildHandle {
            node: Box::new(child),
        };

        let pod = PodNode::new(
            AgentId::from("default/orchestrator"),
            vec![],
            vec![handle],
            make_pod_config(),
            PathBuf::from("/tmp/test"),
            PermissionMode::DontAsk,
        );

        let children = pod.children();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].0, "default/coder");
    }

    #[tokio::test]
    async fn pod_lifecycle() {
        let pod = PodNode::new(
            AgentId::from("default/orchestrator"),
            vec![],
            vec![],
            make_pod_config(),
            PathBuf::from("/tmp/test"),
            PermissionMode::DontAsk,
        );

        assert_eq!(pod.status().await, AgentStatus::Created);

        pod.suspend("test").await.unwrap();
        assert_eq!(pod.status().await, AgentStatus::Suspended);

        pod.resume().await.unwrap();
        assert_eq!(pod.status().await, AgentStatus::Ready);

        pod.shutdown().await.unwrap();
        assert_eq!(pod.status().await, AgentStatus::Stopped);
    }

    #[test]
    fn build_tree_from_gateway_config() {
        let cluster_config = crate::control::topology::ClusterConfig {
            cluster: crate::control::topology::ClusterContext {
                name: Some("test-cluster".to_string()),
                namespace: Some("test-ns".to_string()),
                work_dir: Some(PathBuf::from("/workspace")),
                ..Default::default()
            },
            defaults: crate::control::topology::DefaultsConfig {
                model: Some("claude-3-5-sonnet-latest".to_string()),
                ..Default::default()
            },
            models: std::collections::HashMap::new(),
            agents: vec![crate::control::topology::AgentConfig {
                kind: Some(crate::control::topology::AgentKind::Gateway),
                name: Some("gateway".to_string()),
                workers: vec![crate::control::topology::AgentConfig {
                    kind: Some(crate::control::topology::AgentKind::Worker),
                    name: Some("coder".to_string()),
                    model: Some("claude-3-haiku".to_string()),
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };

        let base_config = AppConfig {
            api_key: Some("test-key".to_string()),
            ..AppConfig::default()
        };

        let root =
            build_agent_tree(&cluster_config, &base_config, PermissionMode::DontAsk).unwrap();
        assert!(root.is_gateway());
        assert_eq!(root.id().0, "test-ns/gateway");
        assert_eq!(root.children().len(), 1);
    }

    #[test]
    fn build_tree_three_level_nesting() {
        // Gateway -> Pod (orchestrator) -> Worker (coder)
        let cluster_config = crate::control::topology::ClusterConfig {
            cluster: crate::control::topology::ClusterContext {
                name: Some("nested-cluster".to_string()),
                namespace: Some("ns".to_string()),
                work_dir: Some(PathBuf::from("/workspace")),
                ..Default::default()
            },
            defaults: crate::control::topology::DefaultsConfig {
                model: Some("default-model".to_string()),
                ..Default::default()
            },
            models: std::collections::HashMap::new(),
            agents: vec![crate::control::topology::AgentConfig {
                kind: Some(crate::control::topology::AgentKind::Gateway),
                name: Some("gateway".to_string()),
                workers: vec![crate::control::topology::AgentConfig {
                    kind: Some(crate::control::topology::AgentKind::Pod),
                    name: Some("orchestrator".to_string()),
                    master: Some(Box::new(crate::control::topology::AgentConfig {
                        name: Some("architect".to_string()),
                        model: Some("master-model".to_string()),
                        ..Default::default()
                    })),
                    workers: vec![
                        crate::control::topology::AgentConfig {
                            kind: Some(crate::control::topology::AgentKind::Worker),
                            name: Some("coder".to_string()),
                            model: Some("worker-model".to_string()),
                            work_dir: Some(PathBuf::from("/workspace/src")),
                            ..Default::default()
                        },
                        crate::control::topology::AgentConfig {
                            kind: Some(crate::control::topology::AgentKind::Worker),
                            name: Some("reviewer".to_string()),
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };

        let base_config = AppConfig {
            api_key: Some("test-key".to_string()),
            ..AppConfig::default()
        };

        let root =
            build_agent_tree(&cluster_config, &base_config, PermissionMode::DontAsk).unwrap();

        // Root is Gateway
        assert!(root.is_gateway());
        assert_eq!(root.id().0, "ns/gateway");

        // Gateway has 1 child (the Pod)
        let gw_children = root.children();
        assert_eq!(gw_children.len(), 1);
        assert_eq!(gw_children[0].0, "ns/orchestrator");
    }

    #[test]
    fn build_tree_no_agents_returns_error() {
        let cluster_config = crate::control::topology::ClusterConfig::default();
        let base_config = AppConfig::default();
        let result = build_agent_tree(&cluster_config, &base_config, PermissionMode::DontAsk);
        assert!(result.is_err());
    }

    #[test]
    fn convert_access_points_mixed() {
        use crate::control::topology::AccessPointConfig;

        let configs = vec![
            AccessPointConfig::Tui {
                can_approve: Some(true),
            },
            AccessPointConfig::Grpc {
                listen: "127.0.0.1:50052".to_string(),
                auth: None,
                can_approve: Some(false),
            },
        ];

        let aps = convert_access_points(&configs).unwrap();
        assert_eq!(aps.len(), 2);
        assert!(aps[0].can_approve());
        assert!(!aps[1].can_approve());
    }
}
