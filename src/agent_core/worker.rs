use async_trait::async_trait;
use std::path::PathBuf;
use tokio::sync::{RwLock, broadcast, mpsc};

use super::access_point::AccessPoint;
use super::agent::{AgentId, AgentNode, AgentStatus, AgentTask, AgentTaskResult};
use crate::config::AppConfig;
use crate::constants::engine::{ACTION_CHANNEL_BUFFER, EVENT_CHANNEL_BUFFER};
use crate::engine::events::{EngineEvent, UserAction};
use crate::permissions::PermissionMode;

/// LlmWorkerNode — 叶子节点，复用 `KezenEngine` 进行 LLM 推理。
///
/// Worker 没有子节点。收到 `assign()` 后，将任务指令通过 `action_tx`
/// 发送到内部 KezenEngine，引擎在独立 tokio task 上运行 agentic loop。
///
/// ## Channel 架构
///
/// ```text
///   action_tx ──►  action_rx (Engine.run 消费)
///                       │
///   event_tx  ◄──  Engine 产生事件
///       │
///   subscribe() → 上级节点 / routing_loop 订阅
/// ```
///
/// `action_tx` / `action_rx` / `event_tx` 在 `new()` 时创建一次，
/// `init()` 时从 `action_rx` take 出来传给 KezenEngine 构造函数。
/// 整个生命周期只有一对 channel，不存在断线问题。
pub struct LlmWorkerNode {
    id: AgentId,
    access_points: Vec<AccessPoint>,
    status: RwLock<AgentStatus>,

    /// 用于向内部 KezenEngine 发送指令的 channel sender。
    /// 上级节点通过 `action_sender()` 获取 clone。
    action_tx: mpsc::Sender<UserAction>,
    /// action_rx 存在 Option 中，init() 时 take() 出来传给 Engine。
    /// 一旦 take 就不可再次初始化。
    action_rx: RwLock<Option<mpsc::Receiver<UserAction>>>,
    /// 用于上级订阅引擎事件的 broadcast sender。
    /// Engine 直接持有此 sender 的 clone，产生事件时 send 到这里。
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
    /// channel pair 在此时创建一次。引擎不会立即启动——需要调用 `init()`。
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

    /// 获取事件广播的订阅 receiver。
    /// 上级节点（Pod/Gateway routing_loop）通过此方法订阅 Worker 的事件流。
    pub fn subscribe_events(&self) -> broadcast::Receiver<EngineEvent> {
        self.event_tx.subscribe()
    }

    /// 获取 action sender 的克隆，用于向此 Worker 发送指令。
    /// routing_loop 通过此方法直接发送 UserAction 到 Worker 的 Engine。
    pub fn action_sender(&self) -> mpsc::Sender<UserAction> {
        self.action_tx.clone()
    }

    /// 获取 event broadcast sender 的克隆。
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
            api_key: Some(secrecy::SecretString::from("test-key")),
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
