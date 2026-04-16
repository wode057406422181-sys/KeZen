use async_trait::async_trait;
use tokio::sync::{RwLock, broadcast, mpsc};

use super::access_point::{AccessPoint, AccessPointHandle, start_access_point};
use super::agent::{AgentId, AgentNode, AgentStatus, AgentTask, AgentTaskResult};
use super::bus;
use super::master::ChildHandle;
use crate::engine::events::{EngineEvent, UserAction};

/// GatewayNode — Unified access and proxy bridge node.
///
/// Gateway is an Access Point Only node: **No Engine**, no LLM inference.
/// Its sole responsibilities are:
///   1. Route upstream external inputs (TUI / REPL / gRPC Client) to the **downstream backend**.
///   2. Broadcast events from the downstream backend to all upstream access points.
///
/// Downstream backend (`GatewayBackend`) has two modes:
///   - `Children`: Route to local child nodes.
///   - `Proxy`: Proxy forward to a remote Host.
///
/// ## Unified Channel Architecture
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
/// - All access points share a clone of the same `action_tx` (fan-in)
/// - Each access point holds an independent `event_tx.subscribe()` receiver (fan-out)
/// - Backend Loop consumes `action_rx` and produces `event_tx`

/// Gateway's downstream backend — Determines the consumer of action_rx and producer of event_tx.
#[derive(Debug, Clone, PartialEq)]
pub enum GatewayBackend {
    /// Local child nodes routing.
    Children,
    /// Remote gRPC proxy.
    Proxy { target_url: String },
}

pub struct GatewayNode {
    id: AgentId,
    /// Gateway backend mode: decides whether traffic is routed to local children or proxied to remote.
    backend: GatewayBackend,
    /// Configured access points list (used to create handles on start).
    access_points: Vec<AccessPoint>,
    /// Launched access point handles (populated after init).
    active_handles: RwLock<Vec<AccessPointHandle>>,
    /// Child node IDs (used for AgentNode::children()).
    children_ids: Vec<AgentId>,
    /// Child node instances (used in init, taken by routing_loop).
    children_handles: RwLock<Vec<ChildHandle>>,
    /// Running status.
    status: RwLock<AgentStatus>,
    /// AbortHandle for the backend loop (aborted on shutdown).
    backend_abort_handle: RwLock<Option<tokio::task::AbortHandle>>,

    /// UserAction from all access points converges to this sender.
    action_tx: mpsc::Sender<UserAction>,
    /// Gateway action receiver — consumed by Backend Loop.
    action_rx: RwLock<Option<mpsc::Receiver<UserAction>>>,
    /// Event broadcast sender — routing_loop forwards child events here.
    event_tx: broadcast::Sender<EngineEvent>,
}

impl GatewayNode {
    /// Constructs a new GatewayNode.
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

    /// Sets the child node handles.
    /// Called in build_agent_tree() — injects child nodes after Gateway construction.
    pub fn set_children(&mut self, children: Vec<ChildHandle>) {
        self.children_ids = children.iter().map(|ch| ch.node.id().clone()).collect();
        self.children_handles = RwLock::new(children);
    }

    /// Constructs GatewayNode from TOML AgentConfig.
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
            // Proxy Gateway does not allow REPL/TUI
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

    /// Gets the Gateway's event broadcast sender.
    pub fn event_sender(&self) -> broadcast::Sender<EngineEvent> {
        self.event_tx.clone()
    }

    /// Take the action_rx (one-shot). routing_loop uses this method to get the receiver.
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

    /// Checks if a specific kind of access point is configured
    pub fn has_access_point_of_kind(&self, kind: &str) -> bool {
        self.access_points.iter().any(|ap| ap.kind_label() == kind)
    }

    /// Starts the Gateway's backend consumption loop.
    ///
    /// This method spawns a tokio task to consume action_rx.
    /// In Children mode, routes commands to child nodes; in Proxy mode, proxies commands remotely.
    ///
    /// Returns a JoinHandle, callers can await it to wait for the routing loop to exit.
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
                                    // Support streaming: send instructions directly to child node channel
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

                                    // Loop to read events emitted by child node, while listening and forwarding new actions (e.g. Cancel)
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
                                    // Fallback for non-streaming: block and wait for completion using assign()
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

                    // Return children ownership for gateway.shutdown() to use
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
                    Vec::new() // Proxy mode has no children to return
                })
            }
        };

        *self.backend_abort_handle.write().await = Some(join_handle.abort_handle());
        join_handle
    }

    /// Runs foreground access point (blocks main task).
    ///
    /// - If REPL configured → Starts REPL (blocks until user exits)
    /// - If TUI configured → Starts TUI (blocks until user exits)
    /// - If neither (gRPC only) → Waits for tokio::signal::ctrl_c()
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
            // gRPC-only mode: Wait for Ctrl+C
            eprintln!("Gateway running (gRPC only). Press Ctrl+C to stop.");
            tokio::signal::ctrl_c().await?;
        }

        Ok(())
    }

    /// Returns the number of active access points.
    pub async fn active_access_point_count(&self) -> usize {
        self.active_handles.read().await.len()
    }

    /// Checks if any access point has approval authority.
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

        // 1. Initialize backend dependencies
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
                // Proxy mode has no local children to initialize
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
        // Regardless of backend, send task via channel.
        // Note: Current implementation assumes serial single task. If there are concurrent assign() calls,
        // multiple subscribers will receive all broadcast events, causing output cross-contamination.
        self.action_tx
            .send(UserAction::SendMessage {
                content: task.instruction,
            })
            .await?;

        // Collect results until Done/Error
        let mut event_rx = self.event_tx.subscribe();
        let mut output = String::new();
        loop {
            match event_rx.recv().await {
                Ok(EngineEvent::TextDelta { text }) => output.push_str(&text),
                Ok(EngineEvent::Done) => break,
                Ok(EngineEvent::Error { message }) => anyhow::bail!("Task failed: {}", message),
                Ok(_) => {} // Ignore other event types (such as ToolUse, etc.)
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

        // 1. Abort backend loop
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
        assert_eq!(
            gw.backend,
            GatewayBackend::Proxy {
                target_url: "http://192.168.1.100:50052".to_string(),
            }
        );
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
