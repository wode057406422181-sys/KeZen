use async_trait::async_trait;
use std::path::PathBuf;
use tokio::sync::{RwLock, broadcast, mpsc};

use super::access_point::AccessPoint;
use super::agent::{AgentId, AgentNode, AgentStatus, AgentTask, AgentTaskResult};
use crate::config::AppConfig;
use crate::constants::engine::{ACTION_CHANNEL_BUFFER, EVENT_CHANNEL_BUFFER};
use crate::engine::events::{EngineEvent, UserAction};
use crate::permissions::PermissionMode;

/// LlmWorkerNode — Leaf node, reuses `KezenEngine` for LLM inference.
///
/// Worker has no child nodes. Upon receiving `assign()`, it passes task instructions
/// via `action_tx` to the internal KezenEngine, which runs the agentic loop on an independent tokio task.
///
/// ## Channel Architecture
///
/// ```text
///   action_tx ──►  action_rx (consumed by Engine.run)
///                       │
///   event_tx  ◄──  Engine generates events
///       │
///   subscribe() → Subscribed by upstream node / routing_loop
/// ```
///
/// `action_tx` / `action_rx` / `event_tx` are created once during `new()`,
/// `init()` takes `action_rx` and passes it to KezenEngine constructor.
/// The entire lifecycle only has one channel pair, no disconnect issues.
pub struct LlmWorkerNode {
    id: AgentId,
    access_points: Vec<AccessPoint>,
    status: RwLock<AgentStatus>,

    /// Channel sender for sending instructions to the internal KezenEngine.
    /// Upstream node gets a clone via `action_sender()`.
    action_tx: mpsc::Sender<UserAction>,
    /// action_rx is kept in Option, taken out during init() to pass to Engine.
    /// Once taken, cannot be initialized again.
    action_rx: RwLock<Option<mpsc::Receiver<UserAction>>>,
    /// Broadcast sender for upstream to subscribe to engine events.
    /// Engine directly holds a clone of this sender, and sends generated events here.
    event_tx: broadcast::Sender<EngineEvent>,

    /// Configuration required to build KezenEngine (used during init()).
    config: AppConfig,
    work_dir: PathBuf,
    permission_mode: PermissionMode,

    /// Engine tokio task handle (populated after init).
    engine_handle: RwLock<Option<tokio::task::JoinHandle<()>>>,
}

impl LlmWorkerNode {
    /// Constructs a new LlmWorkerNode.
    ///
    /// Channel pair is created once at this point. Engine does not start immediately — `init()` must be called.
    pub fn new(
        id: AgentId,
        access_points: Vec<AccessPoint>,
        config: AppConfig,
        work_dir: PathBuf,
        permission_mode: PermissionMode,
    ) -> Self {
        let (action_tx, action_rx) = mpsc::channel(ACTION_CHANNEL_BUFFER);
        let (event_tx, _) = broadcast::channel(EVENT_CHANNEL_BUFFER);

        Self {
            id,
            access_points,
            status: RwLock::new(AgentStatus::Created),
            action_tx,
            action_rx: RwLock::new(Some(action_rx)),
            event_tx,
            config,
            work_dir,
            permission_mode,
            engine_handle: RwLock::new(None),
        }
    }

    /// Gets the subscription receiver for event broadcast.
    /// Upstream node (Master/Gateway routing_loop) uses this method to subscribe to the Worker's event stream.
    pub fn subscribe_events(&self) -> broadcast::Receiver<EngineEvent> {
        self.event_tx.subscribe()
    }

    /// Gets a clone of the action sender, used to send instructions to this Worker.
    /// routing_loop uses this method to directly send UserAction to the Worker's Engine.
    pub fn action_sender(&self) -> mpsc::Sender<UserAction> {
        self.action_tx.clone()
    }

    /// Gets a clone of the event broadcast sender.
    pub fn event_sender(&self) -> broadcast::Sender<EngineEvent> {
        self.event_tx.clone()
    }
}

#[async_trait]
impl AgentNode for LlmWorkerNode {
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
        tracing::info!(agent = %self.id, work_dir = %self.work_dir.display(), "Worker node initializing");

        // Take the action_rx (one-shot: cannot re-init).
        let action_rx = self.action_rx.write().await.take().ok_or_else(|| {
            anyhow::anyhow!("Worker {} already initialized (action_rx taken)", self.id)
        })?;

        let registry =
            crate::tools::registry::create_default_registry(&self.config, self.work_dir.clone());

        let engine = crate::engine::KezenEngine::new(
            self.config.clone(),
            action_rx,
            self.event_tx.clone(),
            registry,
            self.permission_mode,
            self.work_dir.clone(),
        )
        .await
        .map_err(|e| anyhow::anyhow!("Failed to initialize KezenEngine for {}: {}", self.id, e))?;

        // Spawn the engine on a separate task.
        let agent_id = self.id.clone();
        let handle = tokio::spawn(async move {
            tracing::info!(agent = %agent_id, "Worker engine task started");
            engine.run().await;
            tracing::info!(agent = %agent_id, "Worker engine task finished");
        });

        let mut engine_handle = self.engine_handle.write().await;
        *engine_handle = Some(handle);

        let mut status = self.status.write().await;
        *status = AgentStatus::Ready;
        tracing::info!(agent = %self.id, "Worker node ready");
        Ok(())
    }

    async fn assign(&self, task: AgentTask) -> anyhow::Result<AgentTaskResult> {
        tracing::info!(agent = %self.id, task_id = %task.task_id, "Worker received task");

        {
            let mut status = self.status.write().await;
            *status = AgentStatus::Running;
        }

        // Send the task instruction to the engine via the action channel.
        self.action_tx
            .send(UserAction::SendMessage {
                content: task.instruction.clone(),
            })
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send task to engine: {}", e))?;

        // Collect the engine's response by subscribing to events.
        // We wait until we receive EngineEvent::Done, accumulating text deltas.
        let mut event_rx = self.event_tx.subscribe();
        let mut output = String::new();
        let mut had_error = false;

        loop {
            match event_rx.recv().await {
                Ok(EngineEvent::TextDelta { text }) => {
                    output.push_str(&text);
                }
                Ok(EngineEvent::Done) => {
                    break;
                }
                Ok(EngineEvent::Error { message }) => {
                    output.push_str(&format!("\nError: {}", message));
                    had_error = true;
                    break;
                }
                Ok(_) => {
                    // Other events (ToolUseStart, CostUpdate, etc.) — skip for now.
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(agent = %self.id, lagged = n, "Event receiver lagged");
                }
                Err(broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }

        {
            let mut status = self.status.write().await;
            *status = AgentStatus::Ready;
        }

        Ok(AgentTaskResult {
            task_id: task.task_id,
            success: !had_error,
            output,
            data: None,
        })
    }

    async fn suspend(&self, reason: &str) -> anyhow::Result<()> {
        tracing::info!(agent = %self.id, reason = %reason, "Worker suspending");
        let mut status = self.status.write().await;
        *status = AgentStatus::Suspended;
        Ok(())
    }

    async fn resume(&self) -> anyhow::Result<()> {
        tracing::info!(agent = %self.id, "Worker resuming");
        let mut status = self.status.write().await;
        *status = AgentStatus::Ready;
        Ok(())
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        tracing::info!(agent = %self.id, "Worker shutting down");

        let mut handle = self.engine_handle.write().await;
        if let Some(h) = handle.take() {
            h.abort();
            let _ = h.await;
        }

        let mut status = self.status.write().await;
        *status = AgentStatus::Stopped;
        Ok(())
    }

    fn children(&self) -> Vec<AgentId> {
        vec![] // Worker is a leaf node.
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

    fn make_worker_config() -> AppConfig {
        AppConfig {
            model: Some("test-model".to_string()),
            runtime_profile: crate::config::ModelProfile {
                api_key: Some(secrecy::SecretString::from("test-key")),
                ..Default::default()
            },
            ..AppConfig::default()
        }
    }

    #[test]
    fn worker_is_not_gateway() {
        let worker = LlmWorkerNode::new(
            AgentId::from("default/coder"),
            vec![],
            make_worker_config(),
            PathBuf::from("/tmp/test"),
            PermissionMode::DontAsk,
        );
        assert!(!worker.is_gateway());
    }

    #[test]
    fn worker_has_no_children() {
        let worker = LlmWorkerNode::new(
            AgentId::from("default/coder"),
            vec![],
            make_worker_config(),
            PathBuf::from("/tmp/test"),
            PermissionMode::DontAsk,
        );
        assert!(worker.children().is_empty());
    }

    #[test]
    fn worker_id_is_correct() {
        let worker = LlmWorkerNode::new(
            AgentId::from("ns/my-coder"),
            vec![],
            make_worker_config(),
            PathBuf::from("/tmp/test"),
            PermissionMode::DontAsk,
        );
        assert_eq!(worker.id().0, "ns/my-coder");
    }

    #[tokio::test]
    async fn worker_initial_status_is_created() {
        let worker = LlmWorkerNode::new(
            AgentId::from("default/coder"),
            vec![],
            make_worker_config(),
            PathBuf::from("/tmp/test"),
            PermissionMode::DontAsk,
        );
        assert_eq!(worker.status().await, AgentStatus::Created);
    }

    #[tokio::test]
    async fn worker_suspend_resume_lifecycle() {
        let worker = LlmWorkerNode::new(
            AgentId::from("default/coder"),
            vec![],
            make_worker_config(),
            PathBuf::from("/tmp/test"),
            PermissionMode::DontAsk,
        );

        worker.suspend("testing").await.unwrap();
        assert_eq!(worker.status().await, AgentStatus::Suspended);

        worker.resume().await.unwrap();
        assert_eq!(worker.status().await, AgentStatus::Ready);
    }

    #[tokio::test]
    async fn worker_shutdown() {
        let worker = LlmWorkerNode::new(
            AgentId::from("default/coder"),
            vec![],
            make_worker_config(),
            PathBuf::from("/tmp/test"),
            PermissionMode::DontAsk,
        );

        worker.shutdown().await.unwrap();
        assert_eq!(worker.status().await, AgentStatus::Stopped);
    }

    #[tokio::test]
    async fn worker_channel_connected() {
        // Verify action_tx and event_tx are the same pair that init() would use.
        let worker = LlmWorkerNode::new(
            AgentId::from("default/coder"),
            vec![],
            make_worker_config(),
            PathBuf::from("/tmp/test"),
            PermissionMode::DontAsk,
        );

        // Subscribe to events BEFORE init — this is what routing_loop does.
        let mut event_rx = worker.subscribe_events();
        let action_tx = worker.action_sender();

        // Verify the event_tx is functional
        let _ = worker.event_tx.send(EngineEvent::TextDelta {
            text: "hello".to_string(),
        });
        let evt = event_rx.recv().await.unwrap();
        assert!(matches!(evt, EngineEvent::TextDelta { text } if text == "hello"));

        // Verify action_tx can send (even though no engine is running)
        assert!(action_tx.capacity() > 0);
    }
}
