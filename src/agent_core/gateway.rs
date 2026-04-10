use async_trait::async_trait;
use tokio::sync::RwLock;

use super::access_point::AccessPoint;
use super::agent::{AgentId, AgentNode, AgentStatus, AgentTask, AgentTaskResult};

/// GatewayNode — 拓扑树的根节点。
///
/// Gateway 是 Access Point Only 的节点：**没有 Engine**，不做 LLM 推理。
/// 它的全部职责是：
///   1. 将外部输入（TUI / REPL / gRPC）路由到下级 Agent。
///   2. 汇聚下级 Agent 产生的事件并广播到所有接入点。
///   3. 行使准入审批权（通过 `can_approve` 接入点）。
///
/// Gateway -> 一个或多个 Pod/Worker 子节点。
pub struct GatewayNode {
    id: AgentId,
    /// 所有活跃接入点（TUI / REPL / gRPC 服务器句柄等）。
    access_points: Vec<AccessPoint>,
    /// 子节点 ID 列表。
    children: Vec<AgentId>,
    /// 运行状态。
    status: RwLock<AgentStatus>,
}

impl GatewayNode {
    /// 构造一个新的 GatewayNode。
    ///
    /// # Arguments
    /// - `id` — 唯一节点 ID（通常为 `"gateway"` 或 `<namespace>/gateway`）。
    /// - `access_points` — 配置的接入点列表。
    /// - `children` — 下级 Agent 的 ID 列表。
    pub fn new(id: AgentId, access_points: Vec<AccessPoint>, children: Vec<AgentId>) -> Self {
        Self {
            id,
            access_points,
            children,
            status: RwLock::new(AgentStatus::Created),
        }
    }

    /// 从 TOML AgentConfig 构造 GatewayNode。
    ///
    /// 将 `topology::AccessPointConfig` 转换为运行时 `AccessPoint`，
    /// 并收集子节点 ID。
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

        let children: Vec<AgentId> = config
            .workers
            .iter()
            .map(|w| {
                let wname = w.name.as_deref().unwrap_or("unnamed");
                AgentId(format!("{}/{}", ns, wname))
            })
            .collect();

        Ok(Self::new(id, access_points, children))
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
        tracing::info!(agent = %self.id, "Gateway node initializing");
        let mut status = self.status.write().await;
        *status = AgentStatus::Ready;
        tracing::info!(agent = %self.id, children = self.children.len(), "Gateway node ready");
        Ok(())
    }

    async fn assign(&self, task: AgentTask) -> anyhow::Result<AgentTaskResult> {
        // TODO(T-015): Actual routing to child agents via MPSC channels.
        // For now, return an acknowledgement — real routing is implemented
        // when the multi-agent runtime is wired up.
        tracing::info!(
            agent = %self.id,
            task_id = %task.task_id,
            "Gateway received task (routing not yet implemented)"
        );

        Ok(AgentTaskResult {
            task_id: task.task_id,
            success: true,
            output: "Gateway acknowledged task (multi-agent routing pending T-015)".to_string(),
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
        let mut status = self.status.write().await;
        *status = AgentStatus::Stopped;
        Ok(())
    }

    fn children(&self) -> Vec<AgentId> {
        self.children.clone()
    }

    fn is_gateway(&self) -> bool {
        true
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

    #[tokio::test]
    async fn gateway_lifecycle() {
        let gw = make_gateway();

        // Initially Created
        assert_eq!(gw.status().await, AgentStatus::Created);

        // Init -> Ready
        gw.init().await.unwrap();
        assert_eq!(gw.status().await, AgentStatus::Ready);

        // Suspend -> Suspended
        gw.suspend("maintenance").await.unwrap();
        assert_eq!(gw.status().await, AgentStatus::Suspended);

        // Resume -> Ready
        gw.resume().await.unwrap();
        assert_eq!(gw.status().await, AgentStatus::Ready);

        // Shutdown -> Stopped
        gw.shutdown().await.unwrap();
        assert_eq!(gw.status().await, AgentStatus::Stopped);
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

        // Verify gRPC AP
        match &gw.access_points()[2] {
            AccessPoint::Grpc { addr, can_approve } => {
                assert_eq!(addr.to_string(), "127.0.0.1:50052");
                assert!(!can_approve);
            }
            other => panic!("Expected Grpc, got {:?}", other),
        }
    }
}
