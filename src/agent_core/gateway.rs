use async_trait::async_trait;
use tokio::sync::{broadcast, mpsc, RwLock};

use super::access_point::{AccessPoint, AccessPointHandle, start_access_point};
use super::agent::{AgentId, AgentNode, AgentStatus, AgentTask, AgentTaskResult};
use super::bus;
use super::pod::ChildHandle;
use crate::engine::events::{EngineEvent, UserAction};

/// GatewayNode — 拓扑树的根节点。
///
/// Gateway 是 Access Point Only 的节点：**没有 Engine**，不做 LLM 推理。
/// 它的全部职责是：
///   1. 将外部输入（TUI / REPL / gRPC）路由到下级 Agent。
///   2. 汇聚下级 Agent 产生的事件并广播到所有接入点。
///   3. 行使准入审批权（通过 `can_approve` 接入点）。
///
/// ## Channel 架构
///
/// ```text
///   AccessPoint(TUI)  ──┐
///   AccessPoint(REPL) ──┤──► action_tx ──► routing_loop ──► child.action_tx
///   AccessPoint(gRPC) ──┘                     ▲
///                                             │
///   AccessPoint(TUI)  ◄──┐                    │
///   AccessPoint(REPL) ◄──┤◄── event_tx ◄──── child.event_tx.subscribe()
///   AccessPoint(gRPC) ◄──┘
/// ```
///
/// - 所有接入点共享同一个 `action_tx` 的 clone（汇入）
/// - 所有接入点各自持有独立的 `event_tx.subscribe()` receiver（扇出）
/// - routing_loop 从 `action_rx` 读取消息，路由到子节点
pub struct GatewayNode {
    id: AgentId,
    /// 配置的接入点列表（用于启动时创建 handle）。
    access_points: Vec<AccessPoint>,
    /// 已启动的接入点 handle（init 后填充）。
    active_handles: RwLock<Vec<AccessPointHandle>>,
    /// 子节点 ID 列表（用于 AgentNode::children()）。
    children_ids: Vec<AgentId>,
    /// 子节点实例列表（init 时使用，routing_loop 时 take 走）。
    children_handles: RwLock<Vec<ChildHandle>>,
    /// 运行状态。
    status: RwLock<AgentStatus>,

    /// 所有接入点的 UserAction 汇入此 sender。
    action_tx: mpsc::Sender<UserAction>,
    /// Gateway 的 action receiver—— routing_loop 消费。
    action_rx: RwLock<Option<mpsc::Receiver<UserAction>>>,
    /// 事件广播 sender——routing_loop 将子节点事件转发到此处。
    event_tx: broadcast::Sender<EngineEvent>,
}

impl GatewayNode {
    /// 构造一个新的 GatewayNode。
    pub fn new(id: AgentId, access_points: Vec<AccessPoint>, children_ids: Vec<AgentId>) -> Self {
        let channels = bus::create_default_channel_pair();

        Self {
            id,
            access_points,
            active_handles: RwLock::new(Vec::new()),
            children_ids,
            children_handles: RwLock::new(Vec::new()),
            status: RwLock::new(AgentStatus::Created),
            action_tx: channels.action_tx,
            action_rx: RwLock::new(channels.action_rx),
            event_tx: channels.event_tx,
        }
    }

    /// 设置子节点句柄列表。
    /// 在 build_agent_tree() 中调用——Gateway 构建完成后注入子节点。
    pub fn set_children(&mut self, children: Vec<ChildHandle>) {
        self.children_ids = children.iter().map(|ch| ch.node.id().clone()).collect();
        self.children_handles = RwLock::new(children);
    }

    /// 从 TOML AgentConfig 构造 GatewayNode。
    pub fn from_config(
        config: &crate::control::topology::AgentConfig,
        namespace: Option<&str>,
    ) -> anyhow::Result<Self> {
        let ns = namespace.unwrap_or("default");
        let name = config.name.as_deref().unwrap_or("gateway");
        let id = AgentId(format!("{}/{}", ns, name));

        let mut access_points = Vec::new();
        for ap_config in &config.access_points {
            match ap_config {
                crate::control::topology::AccessPointConfig::Tui { can_approve } => {
                    access_points.push(AccessPoint::Tui {
                        can_approve: can_approve.unwrap_or(true),
                    });
                }
                crate::control::topology::AccessPointConfig::Repl { can_approve } => {
                    access_points.push(AccessPoint::Repl {
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
                    access_points.push(AccessPoint::Grpc {
                        addr,
                        can_approve: can_approve.unwrap_or(false),
                    });
                }
            }
        }

        // Collect child IDs from config (actual ChildHandles set later via set_children).
        let children_ids: Vec<AgentId> = config
            .workers
            .iter()
            .map(|w| {
                let wname = w.name.as_deref().unwrap_or("unnamed");
                AgentId(format!("{}/{}", ns, wname))
            })
            .collect();

        Ok(Self::new(id, access_points, children_ids))
    }

    /// 获取 Gateway 的 event broadcast sender。
    pub fn event_sender(&self) -> broadcast::Sender<EngineEvent> {
        self.event_tx.clone()
    }

    /// 获取 Gateway 的 action sender 的克隆。
    pub fn action_sender(&self) -> mpsc::Sender<UserAction> {
        self.action_tx.clone()
    }

    /// Take the action_rx (one-shot). routing_loop 使用此方法获取 receiver。
    pub async fn take_action_rx(&self) -> mpsc::Receiver<UserAction> {
        self.action_rx
            .write()
            .await
            .take()
            .expect("GatewayNode action_rx already taken")
    }

    /// Take all child handles (one-shot). routing_loop 使用此方法获取子节点。
    pub async fn take_children(&self) -> Vec<ChildHandle> {
        std::mem::take(&mut *self.children_handles.write().await)
    }

    /// 返回已激活的接入点数量。
    pub async fn active_access_point_count(&self) -> usize {
        self.active_handles.read().await.len()
    }

    /// 检查是否有任何接入点拥有审批权。
    pub fn has_approval_authority(&self) -> bool {
        self.access_points.iter().any(|ap| ap.can_approve())
    }
}

#[async_trait]
impl AgentNode for GatewayNode {
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
        tracing::info!(
            agent = %self.id,
            access_points = self.access_points.len(),
            children = self.children_ids.len(),
            "Gateway node initializing"
        );

        // 1. Init all children first (Workers spawn engines, Pods recurse).
        let children = self.children_handles.read().await;
        for child in children.iter() {
            child.node.init().await.map_err(|e| {
                anyhow::anyhow!("Failed to init child {} of gateway {}: {}", child.node.id(), self.id, e)
            })?;
            tracing::info!(agent = %self.id, child = %child.node.id(), "Child initialized");
        }
        drop(children);

        // 2. Start access points.
        let mut handles = self.active_handles.write().await;
        for ap in &self.access_points {
            match start_access_point(ap, self.action_tx.clone(), self.event_tx.clone()).await {
                Ok(handle) => {
                    tracing::info!(
                        agent = %self.id,
                        kind = ap.kind_label(),
                        can_approve = ap.can_approve(),
                        "Access point started"
                    );
                    handles.push(handle);
                }
                Err(e) => {
                    tracing::warn!(
                        agent = %self.id, kind = ap.kind_label(), error = %e,
                        "Failed to start access point"
                    );
                }
            }
        }

        let mut status = self.status.write().await;
        *status = AgentStatus::Ready;
        tracing::info!(agent = %self.id, active_aps = handles.len(), "Gateway node ready");
        Ok(())
    }

    async fn assign(&self, task: AgentTask) -> anyhow::Result<AgentTaskResult> {
        // Gateway 的 assign() 不做路由——路由在 routing_loop 中实现。
        // 此方法主要用于测试和非交互场景。
        tracing::info!(agent = %self.id, task_id = %task.task_id, "Gateway assign (no-op)");

        Ok(AgentTaskResult {
            task_id: task.task_id,
            success: true,
            output: "Gateway acknowledged task".to_string(),
            data: None,
        })
    }

    async fn suspend(&self, reason: &str) -> anyhow::Result<()> {
        tracing::info!(agent = %self.id, reason = %reason, "Gateway suspending");
        let mut status = self.status.write().await;
        *status = AgentStatus::Suspended;
        Ok(())
    }

    async fn resume(&self) -> anyhow::Result<()> {
        tracing::info!(agent = %self.id, "Gateway resuming");
        let mut status = self.status.write().await;
        *status = AgentStatus::Ready;
        Ok(())
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        tracing::info!(agent = %self.id, "Gateway shutting down");

        // 1. Shutdown all children.
        let children = self.children_handles.read().await;
        for child in children.iter() {
            if let Err(e) = child.node.shutdown().await {
                tracing::warn!(child = %child.node.id(), error = %e, "Child shutdown error");
            }
        }
        drop(children);

        // 2. Abort all active access points.
        let mut handles = self.active_handles.write().await;
        for handle in handles.iter_mut() {
            handle.abort();
        }
        handles.clear();

        let mut status = self.status.write().await;
        *status = AgentStatus::Stopped;
        Ok(())
    }

    fn children(&self) -> Vec<AgentId> {
        self.children_ids.clone()
    }

    fn is_gateway(&self) -> bool {
        true
    }

    fn into_any(self: Box<Self>) -> Box<dyn std::any::Any> {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_gateway() -> GatewayNode {
        GatewayNode::new(
            AgentId::from("default/gateway"),
            vec![
                AccessPoint::Tui { can_approve: true },
                AccessPoint::Repl { can_approve: true },
            ],
            vec![
                AgentId::from("default/orchestrator"),
                AgentId::from("default/test-crew"),
            ],
        )
    }

    #[test]
    fn gateway_is_gateway() {
        let gw = make_gateway();
        assert!(gw.is_gateway());
    }

    #[test]
    fn gateway_id() {
        let gw = make_gateway();
        assert_eq!(gw.id().0, "default/gateway");
    }

    #[test]
    fn gateway_children() {
        let gw = make_gateway();
        let children = gw.children();
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].0, "default/orchestrator");
    }

    #[test]
    fn gateway_access_points() {
        let gw = make_gateway();
        let aps = gw.access_points();
        assert_eq!(aps.len(), 2);
        assert!(aps[0].can_approve());
    }

    #[test]
    fn gateway_has_approval_authority() {
        let gw = make_gateway();
        assert!(gw.has_approval_authority());

        let gw_no_approve = GatewayNode::new(
            AgentId::from("test/gw"),
            vec![AccessPoint::Repl { can_approve: false }],
            vec![],
        );
        assert!(!gw_no_approve.has_approval_authority());
    }

    #[tokio::test]
    async fn gateway_lifecycle() {
        let gw = make_gateway();
        assert_eq!(gw.status().await, AgentStatus::Created);

        gw.init().await.unwrap();
        assert_eq!(gw.status().await, AgentStatus::Ready);
        assert_eq!(gw.active_access_point_count().await, 2);

        gw.suspend("maintenance").await.unwrap();
        assert_eq!(gw.status().await, AgentStatus::Suspended);

        gw.resume().await.unwrap();
        assert_eq!(gw.status().await, AgentStatus::Ready);

        gw.shutdown().await.unwrap();
        assert_eq!(gw.status().await, AgentStatus::Stopped);
        assert_eq!(gw.active_access_point_count().await, 0);
    }

    #[tokio::test]
    async fn gateway_assign_returns_ack() {
        let gw = make_gateway();
        gw.init().await.unwrap();

        let task = AgentTask {
            task_id: "test-task-001".to_string(),
            instruction: "Build the project".to_string(),
            sender: None,
            context: None,
        };
        let result = gw.assign(task).await.unwrap();
        assert!(result.success);
        assert_eq!(result.task_id, "test-task-001");
    }

    #[test]
    fn gateway_from_config() {
        let toml_str = r#"
        kind = "Gateway"
        name = "my-gateway"

          [[access_points]]
          type = "tui"
          can_approve = true

          [[access_points]]
          type = "repl"
          can_approve = true

          [[access_points]]
          type = "grpc"
          listen = "127.0.0.1:50052"
          can_approve = false

          [[workers]]
          kind = "Pod"
          name = "orchestrator"

          [[workers]]
          kind = "Worker"
          name = "coder"
        "#;

        let agent_config: crate::control::topology::AgentConfig =
            toml::from_str(toml_str).unwrap();
        let gw = GatewayNode::from_config(&agent_config, Some("test-ns")).unwrap();

        assert_eq!(gw.id().0, "test-ns/my-gateway");
        assert!(gw.is_gateway());
        assert_eq!(gw.access_points().len(), 3);
        assert_eq!(gw.children().len(), 2);
        assert!(gw.has_approval_authority());

        match &gw.access_points()[2] {
            AccessPoint::Grpc { addr, can_approve } => {
                assert_eq!(addr.to_string(), "127.0.0.1:50052");
                assert!(!can_approve);
            }
            other => panic!("Expected Grpc, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn gateway_event_broadcast_reaches_subscribers() {
        let gw = make_gateway();
        let mut rx1 = gw.event_tx.subscribe();
        let mut rx2 = gw.event_tx.subscribe();

        let _ = gw.event_tx.send(EngineEvent::TextDelta {
            text: "shared".to_string(),
        });

        let e1 = rx1.recv().await.unwrap();
        let e2 = rx2.recv().await.unwrap();
        assert!(matches!(e1, EngineEvent::TextDelta { text } if text == "shared"));
        assert!(matches!(e2, EngineEvent::TextDelta { text } if text == "shared"));
    }

    #[tokio::test]
    async fn gateway_actions_merge() {
        let gw = make_gateway();
        let mut rx = gw.take_action_rx().await;

        let tx1 = gw.action_sender();
        let tx2 = gw.action_sender();

        tx1.send(UserAction::SendMessage { content: "a".to_string() }).await.unwrap();
        tx2.send(UserAction::SendMessage { content: "b".to_string() }).await.unwrap();

        let a1 = rx.recv().await.unwrap();
        let a2 = rx.recv().await.unwrap();
        assert_eq!(a1, UserAction::SendMessage { content: "a".to_string() });
        assert_eq!(a2, UserAction::SendMessage { content: "b".to_string() });
    }

    #[tokio::test]
    async fn can_approve_propagates() {
        let toml_str = r#"
        kind = "Gateway"
        name = "gw"

          [[access_points]]
          type = "tui"
          can_approve = true

          [[access_points]]
          type = "grpc"
          listen = "127.0.0.1:50099"
          can_approve = false
        "#;

        let agent_config: crate::control::topology::AgentConfig =
            toml::from_str(toml_str).unwrap();
        let gw = GatewayNode::from_config(&agent_config, Some("ns")).unwrap();

        assert!(gw.access_points()[0].can_approve());
        assert!(!gw.access_points()[1].can_approve());
        assert!(gw.has_approval_authority());
    }
}
