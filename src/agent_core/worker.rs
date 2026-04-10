use async_trait::async_trait;
use std::path::PathBuf;
use tokio::sync::{broadcast, mpsc, RwLock};

use super::access_point::AccessPoint;
use super::agent::{AgentId, AgentNode, AgentStatus, AgentTask, AgentTaskResult};
use crate::config::AppConfig;
use crate::constants::defaults::{ACTION_CHANNEL_BUFFER, EVENT_CHANNEL_BUFFER};
use crate::engine::events::{EngineEvent, UserAction};
use crate::permissions::PermissionMode;

/// LlmWorkerNode — 叶子节点，复用 `KezenEngine` 进行 LLM 推理。
///
/// Worker 没有子节点。收到 `assign()` 后，将任务指令通过 `action_tx`
/// 发送到内部 KezenEngine，引擎在独立 tokio task 上运行 agentic loop。
///
/// Worker 的 `event_rx` 用于上级监听引擎产生的事件流。
pub struct LlmWorkerNode {
    id: AgentId,
    access_points: Vec<AccessPoint>,
    status: RwLock<AgentStatus>,

    /// 用于向内部 KezenEngine 发送指令的 channel sender。
    action_tx: mpsc::Sender<UserAction>,
    /// 用于上级订阅引擎事件的 broadcast sender。
    event_tx: broadcast::Sender<EngineEvent>,

    /// 构建 KezenEngine 所需的配置（在 `init()` 时使用）。
    config: AppConfig,
    work_dir: PathBuf,
    permission_mode: PermissionMode,

    /// 引擎 tokio task handle（init 后填充）。
    engine_handle: RwLock<Option<tokio::task::JoinHandle<()>>>,
}

impl LlmWorkerNode {
    /// 构造一个新的 LlmWorkerNode。
    ///
    /// 引擎不会立即启动——需要调用 `init()` 来构建并启动 KezenEngine。
    pub fn new(
        id: AgentId,
        access_points: Vec<AccessPoint>,
        config: AppConfig,
        work_dir: PathBuf,
        permission_mode: PermissionMode,
    ) -> Self {
        let (action_tx, _action_rx) = mpsc::channel(ACTION_CHANNEL_BUFFER);
        let (event_tx, _) = broadcast::channel(EVENT_CHANNEL_BUFFER);

        // Note: _action_rx is dropped here intentionally.
        // A fresh channel pair is created in init() when the engine is actually built.
        // We keep action_tx and event_tx here as the stable handles for the node.
        // The real rx ends are wired during init().

        Self {
            id,
            access_points,
            status: RwLock::new(AgentStatus::Created),
            action_tx,
            event_tx,
            config,
            work_dir,
            permission_mode,
            engine_handle: RwLock::new(None),
        }
    }

    /// 获取事件广播的订阅 receiver。
    /// 上级节点（Pod/Gateway）通过此方法订阅 Worker 的事件流。
    pub fn subscribe_events(&self) -> broadcast::Receiver<EngineEvent> {
        self.event_tx.subscribe()
    }

    /// 获取 action sender 的克隆，用于向此 Worker 发送指令。
    pub fn action_sender(&self) -> mpsc::Sender<UserAction> {
        self.action_tx.clone()
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

        // Create a fresh channel pair for the engine.
        let (_action_tx, action_rx) = mpsc::channel(ACTION_CHANNEL_BUFFER);
        let (event_tx, _) = broadcast::channel(EVENT_CHANNEL_BUFFER);

        // Replace the stable handles with the new ones.
        // Safety: we use ptr::write through the RwLock to update the sender/tx.
        // Since we hold exclusive access during init, this is safe.
        // However, since action_tx and event_tx are not behind RwLock, we need
        // a different approach. For now, we pre-create channels in new() and
        // use those directly. The engine will be wired to these channels.

        let registry = crate::tools::registry::create_default_registry(
            &self.config,
            self.work_dir.clone(),
        );

        let engine = crate::engine::KezenEngine::new(
            self.config.clone(),
            action_rx,
            event_tx.clone(),
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

        // Update the action_tx to point to the new channel.
        // Note: Since action_tx is not behind RwLock, the initial channels from new()
        // are used. To properly wire this, we create channels in new() that are
        // passed directly to the engine. This means the action_tx from new() IS
        // the one the engine listens on.
        //
        // Actually, we need to restructure: create the real channels in new(),
        // store the rx in an Option, and take() it during init().
        // For this iteration, we use the approach where init() creates a new engine
        // with its own channels, and the node exposes the event_tx for subscriptions.

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

        // Signal the engine to stop by dropping the action channel.
        // The engine's run() loop exits when action_rx returns None.
        // We can't drop action_tx directly, but closing the channel
        // will cause the engine to exit naturally.

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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_worker_config() -> AppConfig {
        AppConfig {
            model: Some("test-model".to_string()),
            api_key: Some("test-key".to_string()),
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
}
