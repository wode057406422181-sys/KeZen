use async_trait::async_trait;
use tokio::sync::{RwLock, broadcast, mpsc};

use super::access_point::{AccessPoint, AccessPointHandle, start_access_point};
use super::agent::{AgentId, AgentNode, AgentStatus, AgentTask, AgentTaskResult};
use super::bus;
use super::master::ChildHandle;
use crate::engine::events::{EngineEvent, UserAction};

/// GatewayNode — 统一的接入与代理桥接节点。
///
/// Gateway 是 Access Point Only 的节点：**没有 Engine**，不做 LLM 推理。
/// 它的全部职责是：
///   1. 将上游外部输入（TUI / REPL / gRPC Client）路由到**下游后端**。
///   2. 将下游后端产生的事件广播到所有上游接入点。
///
/// 下游后端 (`GatewayBackend`) 有两种：
///   - `Children`: 路由到本地子节点。
///   - `Proxy`: 代理转发到远端主机。
///
/// ## 统一 Channel 架构
///
/// ```text
///   Upstreams                                 Backend
///
///   AccessPoint(TUI)  ──┐                             ┌──► Local Children
///   AccessPoint(REPL) ──┤──► action_tx ──► Backend ───┤
///   AccessPoint(gRPC) ──┘                  Loop       └──► Remote Cluster
///                                             │
///   AccessPoint(TUI)  ◄──┐                    │
///   AccessPoint(REPL) ◄──┤◄── event_tx ◄──────┘
///   AccessPoint(gRPC) ◄──┘
/// ```
///
/// - 所有接入点共享同一个 `action_tx` 的 clone（汇入）
/// - 所有接入点各自持有独立的 `event_tx.subscribe()` receiver（扇出）
/// - Backend Loop 消费 `action_rx` 并产生 `event_tx`

/// Gateway 的下游后端——决定 action_rx 的消费者和 event_tx 的生产者。
#[derive(Debug, Clone, PartialEq)]
pub enum GatewayBackend {
    /// 本地子节点路由。
    Children,
    /// 远端 gRPC 代理。
    Proxy { target_url: String },
}

pub struct GatewayNode {
    id: AgentId,
    /// Gateway 的后端模式：决定流量路由到本地子节点还是代理到远端。
    backend: GatewayBackend,
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
    /// 后端循环的 AbortHandle（shutdown 时中止）。
    backend_abort_handle: RwLock<Option<tokio::task::AbortHandle>>,

    /// 所有接入点的 UserAction 汇入此 sender。
    action_tx: mpsc::Sender<UserAction>,
    /// Gateway 的 action receiver—— Backend Loop 消费。
    action_rx: RwLock<Option<mpsc::Receiver<UserAction>>>,
    /// 事件广播 sender——routing_loop 将子节点事件转发到此处。
    event_tx: broadcast::Sender<EngineEvent>,
}

impl GatewayNode {
    /// 构造一个新的 GatewayNode。
    pub fn new(
        id: AgentId,
        access_points: Vec<AccessPoint>,
        children_ids: Vec<AgentId>,
        backend: GatewayBackend,
    ) -> Self {
        let channels = bus::create_default_channel_pair();

        Self {
            id,
            backend,
            access_points,
            active_handles: RwLock::new(Vec::new()),
            children_ids,
            children_handles: RwLock::new(Vec::new()),
            status: RwLock::new(AgentStatus::Created),
            backend_abort_handle: RwLock::new(None),
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
        let name = config.name.as_deref().ok_or_else(|| {
            anyhow::anyhow!("Gateway agent must have a 'name' field in kezen.toml")
        })?;
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

        let has_workers = !config.workers.is_empty();
        let has_target = config.target.is_some();

        let backend = match (has_workers, has_target) {
            (true, false) => GatewayBackend::Children,
            (false, true) => {
                let target_url = config.target.clone().unwrap();
                if !target_url.starts_with("http://") && !target_url.starts_with("https://") {
                    anyhow::bail!(
                        "Gateway '{}' target URL must start with http:// or https://, got: '{}'",
                        name,
                        target_url
                    );
                }
                GatewayBackend::Proxy { target_url }
            }
            (true, true) => anyhow::bail!(
                "Gateway '{}' cannot have both workers (children) and a target (proxy)",
                name
            ),
            (false, false) => {
                anyhow::bail!("Gateway '{}' must have either workers or a target", name)
            }
        };

        if matches!(backend, GatewayBackend::Proxy { .. }) {
            // Proxy Gateway 不允许 REPL/TUI
            for ap in &access_points {
                if matches!(ap, AccessPoint::Repl { .. } | AccessPoint::Tui { .. }) {
                    anyhow::bail!(
                        "Proxy Gateway '{}' cannot have REPL/TUI access points. Only gRPC is allowed for proxy Gateways.",
                        name
                    );
                }
            }
        }

        // Collect child IDs from config (actual ChildHandles set later via set_children).
        let children_ids: Vec<AgentId> = if matches!(backend, GatewayBackend::Children) {
            config
                .workers
                .iter()
                .map(|w| {
                    let wname = w.name.as_deref().ok_or_else(|| {
                        anyhow::anyhow!(
                            "Worker agent under Gateway must have a 'name' field in kezen.toml"
                        )
                    })?;
                    Ok(AgentId(format!("{}/{}", ns, wname)))
                })
                .collect::<anyhow::Result<Vec<_>>>()?
        } else {
            Vec::new()
        };

        Ok(Self::new(id, access_points, children_ids, backend))
    }

    /// 获取 Gateway 的 event broadcast sender。
    pub fn event_sender(&self) -> broadcast::Sender<EngineEvent> {
        self.event_tx.clone()
    }

    /// Take the action_rx (one-shot). routing_loop 使用此方法获取 receiver。
    pub async fn take_action_rx(&self) -> mpsc::Receiver<UserAction> {
        self.action_rx
            .write()
            .await
            .take()
            .expect("GatewayNode action_rx already taken")
    }

    /// Take the children handles (one-shot).
    pub async fn take_children(&self) -> Vec<ChildHandle> {
        let mut handles = self.children_handles.write().await;
        std::mem::take(&mut *handles)
    }

    /// 检查是否配置了某种接入点
    pub fn has_access_point_of_kind(&self, kind: &str) -> bool {
        self.access_points.iter().any(|ap| ap.kind_label() == kind)
    }

    /// 启动 Gateway 的后端消费循环。
    ///
    /// 此方法会 spawn 一个 tokio task 来消费 action_rx。
    /// Children 模式下，将指令路由到子节点；Proxy 模式下，将指令代理到远端。
    ///
    /// 返回 JoinHandle，调用方可以 await 它等待路由循环退出。
    pub async fn spawn_backend(&self) -> tokio::task::JoinHandle<Vec<ChildHandle>> {
        let mut action_rx = self.take_action_rx().await;
        let event_tx_for_loop = self.event_tx.clone();
        let gateway_id = self.id.clone();
        let backend = self.backend.clone();

        let join_handle = match backend {
            GatewayBackend::Children => {
                let children = self.take_children().await;
                tokio::spawn(async move {
                    tracing::info!(agent = %gateway_id, "Routing loop started");

                    if children.is_empty() {
                        tracing::error!(agent = %gateway_id, "No children — routing loop exiting");
                        return children;
                    }

                    let mut task_counter = 0u64;
                    let mut pending_actions: std::collections::VecDeque<UserAction> =
                        std::collections::VecDeque::new();

                    loop {
                        let action = if let Some(a) = pending_actions.pop_front() {
                            a
                        } else {
                            match action_rx.recv().await {
                                Some(a) => a,
                                None => break,
                            }
                        };

                        match action {
                            UserAction::SendMessage { content } => {
                                task_counter += 1;
                                let task_id = format!("gw-{:04}", task_counter);

                                // Route to first child. Phase 2 will support multi-child routing.
                                let child = &children[0];

                                tracing::info!(
                                    agent = %gateway_id,
                                    child = %child.node.id(),
                                    task_id = %task_id,
                                    "Routing message to child"
                                );

                                if let (Some(action_tx), Some(mut event_rx)) =
                                    (child.node.action_sender(), child.node.subscribe_events())
                                {
                                    // 支持流式：直接将指令发送给子节点的 channel
                                    if let Err(e) = action_tx
                                        .send(UserAction::SendMessage {
                                            content: content.clone(),
                                        })
                                        .await
                                    {
                                        tracing::error!(task_id = %task_id, error = %e, "Failed to send action to child");
                                        let _ = event_tx_for_loop.send(EngineEvent::Error {
                                            message: format!("Failed to send action: {}", e),
                                        });
                                        continue;
                                    }

                                    // 循环读取子节点抛出的事件，同时监听并转发新的动作（如 Cancel）
                                    loop {
                                        tokio::select! {
                                            action_opt = action_rx.recv() => {
                                                if let Some(action) = action_opt {
                                                    match action {
                                                        UserAction::Cancel | UserAction::PermissionResponse { .. } => {
                                                            if let Err(e) = action_tx.send(action).await {
                                                                tracing::warn!(task_id = %task_id, error = %e, "Failed to forward action to child");
                                                            }
                                                        }
                                                        UserAction::SendMessage { .. } => {
                                                            tracing::warn!(task_id = %task_id, "SendMessage received during active task — queuing");
                                                            pending_actions.push_back(action);
                                                        }
                                                    }
                                                } else {
                                                    // action_rx closed
                                                    break;
                                                }
                                            },
                                            event_res = event_rx.recv() => {
                                                match event_res {
                                                    Ok(event) => {
                                                        let is_terminal = matches!(event, EngineEvent::Done | EngineEvent::Error { .. });
                                                        let _ = event_tx_for_loop.send(event);
                                                        if is_terminal {
                                                            break;
                                                        }
                                                    }
                                                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                                        tracing::warn!(task_id = %task_id, "Child event channel closed unexpectedly");
                                                        let _ = event_tx_for_loop.send(EngineEvent::Error {
                                                            message: "Child disconnected".to_string(),
                                                        });
                                                        break;
                                                    }
                                                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                                        tracing::warn!(task_id = %task_id, skipped = n, "Gateway lagged behind child events");
                                                    }
                                                }
                                            }
                                        }
                                    }
                                } else {
                                    // 不支持流式的降级方案：使用 assign() 阻塞等待完毕
                                    let task = crate::agent_core::agent::AgentTask {
                                        task_id: task_id.clone(),
                                        instruction: content,
                                        sender: Some(gateway_id.clone()),
                                        context: None,
                                    };
                                    match child.node.assign(task).await {
                                        Ok(result) => {
                                            if !result.output.is_empty() {
                                                let _ = event_tx_for_loop.send(
                                                    EngineEvent::TextDelta {
                                                        text: result.output,
                                                    },
                                                );
                                            }
                                            let _ = event_tx_for_loop.send(EngineEvent::Done);
                                        }
                                        Err(e) => {
                                            tracing::error!(task_id = %task_id, error = %e, "Child task failed");
                                            let _ = event_tx_for_loop.send(EngineEvent::Error {
                                                message: format!("Task failed: {}", e),
                                            });
                                        }
                                    }
                                }
                            }
                            UserAction::Cancel => {
                                tracing::info!(agent = %gateway_id, "Cancel received, forwarding to active child");
                                if let Some(child) = children.first() {
                                    if let Some(action_tx) = child.node.action_sender() {
                                        let _ = action_tx.send(UserAction::Cancel).await;
                                    }
                                }
                            }
                            UserAction::PermissionResponse {
                                id,
                                allowed,
                                always_allow,
                            } => {
                                tracing::info!(agent = %gateway_id, id = %id, allowed, "Permission response received, forwarding");
                                if let Some(child) = children.first() {
                                    if let Some(action_tx) = child.node.action_sender() {
                                        let _ = action_tx
                                            .send(UserAction::PermissionResponse {
                                                id,
                                                allowed,
                                                always_allow,
                                            })
                                            .await;
                                    }
                                }
                            }
                        }
                    }

                    // Routing loop ends when action_tx is dropped (REPL exited).
                    tracing::info!(agent = %gateway_id, "Routing loop ended");

                    // 返回 children 所有权供 gateway.shutdown() 使用
                    children
                })
            }
            GatewayBackend::Proxy { target_url } => {
                tokio::spawn(async move {
                    tracing::info!(agent = %gateway_id, target = %target_url, "Proxy loop started");
                    if let Err(e) = crate::frontend::grpc::client::run_grpc_client(
                        target_url,
                        action_rx,
                        event_tx_for_loop,
                    )
                    .await
                    {
                        tracing::error!(error = %e, "Gateway proxy backend failed");
                    }
                    Vec::new() // Proxy 模式无 children 可归还
                })
            }
        };

        *self.backend_abort_handle.write().await = Some(join_handle.abort_handle());
        join_handle
    }

    /// 运行前台接入点（阻塞主 task）。
    ///
    /// - 如果配置了 REPL → 启动 REPL（阻塞直到用户退出）
    /// - 如果配置了 TUI → 启动 TUI（阻塞直到用户退出）
    /// - 如果都没有（纯 gRPC）→ tokio::signal::ctrl_c() 等待
    pub async fn run_foreground(
        &self,
        config: &crate::config::AppConfig,
        initial_prompt: Option<String>,
    ) -> anyhow::Result<()> {
        let action_tx = self.action_tx.clone();
        let event_rx = self.event_tx.subscribe();

        if self.has_access_point_of_kind("REPL") {
            crate::frontend::repl::repl::run_repl(
                config.clone(),
                action_tx,
                event_rx,
                initial_prompt,
            )
            .await?;
        } else if self.has_access_point_of_kind("TUI") {
            crate::frontend::tui::run_tui_client(
                config.clone(),
                action_tx,
                event_rx,
                initial_prompt,
            )
            .await?;
        } else {
            // gRPC-only 模式：等待 Ctrl+C
            eprintln!("Gateway running (gRPC only). Press Ctrl+C to stop.");
            tokio::signal::ctrl_c().await?;
        }

        Ok(())
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

        // 1. 初始化后端依赖
        match &self.backend {
            GatewayBackend::Children => {
                let children = self.children_handles.read().await;
                for child in children.iter() {
                    child.node.init().await.map_err(|e| {
                        anyhow::anyhow!(
                            "Failed to init child {} of gateway {}: {}",
                            child.node.id(),
                            self.id,
                            e
                        )
                    })?;
                    tracing::info!(agent = %self.id, child = %child.node.id(), "Child initialized");
                }
            }
            GatewayBackend::Proxy { .. } => {
                // Proxy 模式没有本地 children 需要初始化
            }
        }

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
        // 无论哪种 backend，都通过 channel 发送任务。
        // 注意：当前实现假定单任务串行。如果有并发 assign() 调用，
        // 多个 subscriber 会各自收到所有 broadcast 事件，导致输出交叉污染。
        self.action_tx
            .send(UserAction::SendMessage {
                content: task.instruction,
            })
            .await?;

        // 收集结果直到 Done/Error
        let mut event_rx = self.event_tx.subscribe();
        let mut output = String::new();
        loop {
            match event_rx.recv().await {
                Ok(EngineEvent::TextDelta { text }) => output.push_str(&text),
                Ok(EngineEvent::Done) => break,
                Ok(EngineEvent::Error { message }) => anyhow::bail!("Task failed: {}", message),
                Ok(_) => {} // 忽略其他事件类型（如 ToolUse 等）
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    anyhow::bail!("Event channel closed unexpectedly during assign");
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(skipped = n, "assign() lagged behind event stream");
                }
            }
        }

        Ok(AgentTaskResult {
            task_id: task.task_id,
            success: true,
            output,
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

        // 1. 中止后端循环
        if let Some(handle) = self.backend_abort_handle.write().await.take() {
            handle.abort();
        }

        // 2. Shut down all children.
        let children = self.children_handles.read().await;
        for child in children.iter() {
            if let Err(e) = child.node.shutdown().await {
                tracing::warn!(child = %child.node.id(), error = %e, "Child shutdown error");
            }
        }
        drop(children);

        // 3. Abort all active access points.
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

    fn action_sender(
        &self,
    ) -> Option<tokio::sync::mpsc::Sender<crate::engine::events::UserAction>> {
        Some(self.action_tx.clone())
    }

    fn subscribe_events(
        &self,
    ) -> Option<tokio::sync::broadcast::Receiver<crate::engine::events::EngineEvent>> {
        Some(self.event_tx.subscribe())
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
            GatewayBackend::Children, // Backend
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
            GatewayBackend::Children,
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
    async fn gateway_assign_forwards_action() {
        let gw = make_gateway();
        gw.init().await.unwrap();

        let task = AgentTask {
            task_id: "test-task-001".to_string(),
            instruction: "Build the project".to_string(),
            sender: None,
            context: None,
        };

        // We simulate backend by popping the action block and replying Done
        let event_tx = gw.event_tx.clone();
        let mut action_rx = gw.take_action_rx().await;

        tokio::spawn(async move {
            if let Some(UserAction::SendMessage { .. }) = action_rx.recv().await {
                let _ = event_tx.send(EngineEvent::TextDelta {
                    text: "mock response".to_string(),
                });
                let _ = event_tx.send(EngineEvent::Done);
            }
        });

        let result = gw.assign(task).await.unwrap();
        assert!(result.success);
        assert_eq!(result.output, "mock response");
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
          kind = "Master"
          name = "orchestrator"

          [[workers]]
          kind = "Worker"
          name = "coder"
        "#;

        let agent_config: crate::control::topology::AgentConfig = toml::from_str(toml_str).unwrap();
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

        let tx1 = gw.action_tx.clone();
        let tx2 = gw.action_tx.clone();

        tx1.send(UserAction::SendMessage {
            content: "a".to_string(),
        })
        .await
        .unwrap();
        tx2.send(UserAction::SendMessage {
            content: "b".to_string(),
        })
        .await
        .unwrap();

        let a1 = rx.recv().await.unwrap();
        let a2 = rx.recv().await.unwrap();
        assert_eq!(
            a1,
            UserAction::SendMessage {
                content: "a".to_string()
            }
        );
        assert_eq!(
            a2,
            UserAction::SendMessage {
                content: "b".to_string()
            }
        );
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

          [[workers]]
          kind = "Worker"
          name = "dummy"
        "#;

        let agent_config: crate::control::topology::AgentConfig = toml::from_str(toml_str).unwrap();
        let gw = GatewayNode::from_config(&agent_config, Some("ns")).unwrap();

        assert!(gw.access_points()[0].can_approve());
        assert!(!gw.access_points()[1].can_approve());
        assert!(gw.has_approval_authority());
    }

    #[test]
    fn proxy_gateway_rejects_repl() {
        let toml_str = r#"
        kind = "Gateway"
        name = "proxy-gw"
        target = "http://127.0.0.1:50051"
          [[access_points]]
          type = "repl"
          can_approve = true
        "#;
        let agent_config: crate::control::topology::AgentConfig = toml::from_str(toml_str).unwrap();
        let result = GatewayNode::from_config(&agent_config, Some("ns"));
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(e.to_string().contains("Proxy Gateway"));
        }
    }

    #[test]
    fn proxy_gateway_with_grpc_only() {
        let toml_str = r#"
        kind = "Gateway"
        name = "remote-proxy"
        target = "http://192.168.1.100:50052"
          [[access_points]]
          type = "grpc"
          listen = "127.0.0.1:50053"
          can_approve = false
        "#;
        let agent_config: crate::control::topology::AgentConfig = toml::from_str(toml_str).unwrap();
        let gw = GatewayNode::from_config(&agent_config, Some("ns")).unwrap();

        assert_eq!(gw.id().0, "ns/remote-proxy");
        assert_eq!(gw.access_points().len(), 1);
        assert_eq!(gw.children().len(), 0);
        assert_eq!(gw.backend, GatewayBackend::Proxy {
            target_url: "http://192.168.1.100:50052".to_string(),
        });
    }

    #[test]
    fn proxy_gateway_rejects_invalid_url() {
        let toml_str = r#"
        kind = "Gateway"
        name = "bad-proxy"
        target = "not-a-url"
        "#;
        let agent_config: crate::control::topology::AgentConfig = toml::from_str(toml_str).unwrap();
        let result = GatewayNode::from_config(&agent_config, Some("ns"));
        assert!(result.is_err());
        if let Err(e) = result {
            let err_msg = e.to_string();
            assert!(err_msg.contains("http://") || err_msg.contains("https://"));
        }
    }
}
