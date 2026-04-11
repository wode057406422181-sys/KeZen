use std::fmt;

use async_trait::async_trait;

use super::access_point::AccessPoint;

/// Unique ID for each Agent tree node.
///
/// Convention format: `<cluster_namespace>/<agent_name>`,
/// for example `default/gateway`, `default/orchestrator/coder`.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct AgentId(pub String);

impl fmt::Display for AgentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for AgentId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// Agent execution status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    /// Agent is created but not yet initialized.
    Created,
    /// Agent is initialized and ready to receive tasks.
    Ready,
    /// Agent is currently executing a task.
    Running,
    /// Agent is suspended (can be resumed).
    Suspended,
    /// Agent is terminated.
    Stopped,
}

impl fmt::Display for AgentStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Created => write!(f, "created"),
            Self::Ready => write!(f, "ready"),
            Self::Running => write!(f, "running"),
            Self::Suspended => write!(f, "suspended"),
            Self::Stopped => write!(f, "stopped"),
        }
    }
}

/// Task assigned to an Agent.
///
/// Generic enough to fit different types of Agents:
/// - Worker: Enters LLM inference loop after receiving `instruction`.
/// - Master: Decomposes into subtasks and distributes to workers upon receipt.
/// - Gateway: Routes to child Agents upon receipt.
#[derive(Debug, Clone)]
pub struct AgentTask {
    /// Unique task ID.
    pub task_id: String,
    /// Task instruction text (natural language description).
    pub instruction: String,
    /// Optional Sender Agent ID (for returning results).
    pub sender: Option<AgentId>,
    /// Optional additional context (structured JSON data).
    pub context: Option<serde_json::Value>,
}

/// Return result after Agent executes a task.
#[derive(Debug, Clone)]
pub struct AgentTaskResult {
    /// Corresponding task ID.
    pub task_id: String,
    /// Whether the task completed successfully.
    pub success: bool,
    /// Result summary / output text.
    pub output: String,
    /// Optional structured result data.
    pub data: Option<serde_json::Value>,
}

/// AgentNode Trait — the core abstraction for multi-agent topology.
///
/// Everything is an Agent: LLM Worker, AI Master, Gateway are all `AgentNode` implementations.
/// The core difference lies only in the behavior of `assign()`:
/// - `LlmWorker` — Calls LLM API for inference
/// - `GatewayNode` — Routes to external input sources (human or remote Agent) via AccessPoint
/// - `Master` — Master decomposes and distributes to child nodes
#[async_trait]
pub trait AgentNode: Send + Sync {
    /// Returns this node's unique ID.
    fn id(&self) -> &AgentId;

    /// Queries current execution status.
    async fn status(&self) -> AgentStatus;

    /// This Agent's [additional] access point list (excluding implicit default access points).
    ///
    /// Default access point rules (no config needed, auto-handled by framework):
    ///   - Non-Root Agent: Parent node is default access point, via in-process MPSC.
    ///   - Root Agent: No parent, events only delivered to access points configured in this list.
    fn access_points(&self) -> &[AccessPoint];

    /// Initializes Agent (connects LLM endpoint, starts TUI/gRPC server, loads Skills, etc.).
    async fn init(&self) -> anyhow::Result<()>;

    /// Assigns a task to this Agent.
    ///
    /// - LlmWorker: Directly enters LLM inference loop.
    /// - GatewayNode: Receives external input via AccessPoint, routes downstream.
    /// - Master: Master decomposes and distributes to child nodes.
    async fn assign(&self, task: AgentTask) -> anyhow::Result<AgentTaskResult>;

    /// Suspends Agent (retains state, pauses processing).
    async fn suspend(&self, reason: &str) -> anyhow::Result<()>;

    /// Resumes from suspended state.
    async fn resume(&self) -> anyhow::Result<()>;

    /// Terminates Agent, releasing all resources.
    async fn shutdown(&self) -> anyhow::Result<()>;

    /// Returns list of child node IDs.
    /// - LlmWorker: Empty list.
    /// - Master / Gateway: Non-empty.
    fn children(&self) -> Vec<AgentId>;

    /// Whether this node is a Gateway (pure access point node without an Engine).
    ///
    /// Framework uses this marker to determine which node "admission approval" requests should route to.
    /// Gateway node has approval authority and can hold access points with `can_approve = true`.
    fn is_gateway(&self) -> bool {
        false
    }

    /// Converts `Box<dyn AgentNode>` to `Box<dyn Any>` for safe downcasting.
    ///
    /// The runtime module needs to downcast the root node to `GatewayNode` to access
    /// specific methods like `take_action_rx()` / `take_children()`.
    fn into_any(self: Box<Self>) -> Box<dyn std::any::Any>;

    /// Gets action sender, returning a clone if this Node supports it
    fn action_sender(
        &self,
    ) -> Option<tokio::sync::mpsc::Sender<crate::engine::events::UserAction>> {
        None
    }

    /// Gets event broadcast subscription receiver, if this Node supports it
    fn subscribe_events(
        &self,
    ) -> Option<tokio::sync::broadcast::Receiver<crate::engine::events::EngineEvent>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_id_display() {
        let id = AgentId("default/gateway".to_string());
        assert_eq!(format!("{}", id), "default/gateway");
    }

    #[test]
    fn agent_id_from_str() {
        let id = AgentId::from("test-agent");
        assert_eq!(id.0, "test-agent");
    }

    #[test]
    fn agent_id_equality() {
        let a = AgentId("same".to_string());
        let b = AgentId("same".to_string());
        assert_eq!(a, b);
    }

    #[test]
    fn agent_status_display() {
        assert_eq!(format!("{}", AgentStatus::Created), "created");
        assert_eq!(format!("{}", AgentStatus::Ready), "ready");
        assert_eq!(format!("{}", AgentStatus::Running), "running");
        assert_eq!(format!("{}", AgentStatus::Suspended), "suspended");
        assert_eq!(format!("{}", AgentStatus::Stopped), "stopped");
    }

    #[test]
    fn agent_task_construction() {
        let task = AgentTask {
            task_id: "task-001".to_string(),
            instruction: "Write unit tests".to_string(),
            sender: Some(AgentId::from("master")),
            context: None,
        };
        assert_eq!(task.task_id, "task-001");
        assert!(task.sender.is_some());
    }

    #[test]
    fn agent_task_result_construction() {
        let result = AgentTaskResult {
            task_id: "task-001".to_string(),
            success: true,
            output: "All tests passed".to_string(),
            data: None,
        };
        assert!(result.success);
        assert_eq!(result.output, "All tests passed");
    }
}
