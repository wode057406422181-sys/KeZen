use std::fmt;

use async_trait::async_trait;

use super::access_point::AccessPoint;

/// 每一个 Agent 树节点的唯一 ID。
///
/// 格式约定：`<cluster_namespace>/<agent_name>`，
/// 例如 `default/gateway`、`default/orchestrator/coder`。
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

/// Agent 运行状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    /// Agent 已创建但尚未初始化。
    Created,
    /// Agent 已初始化，可以接收任务。
    Ready,
    /// Agent 正在执行任务。
    Running,
    /// Agent 已挂起（可恢复）。
    Suspended,
    /// Agent 已终止。
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

/// 分配给 Agent 的任务。
///
/// 足够通用以适配不同类型的 Agent：
/// - Worker：收到 `instruction` 后进入 LLM 推理循环。
/// - Pod Master：收到后拆解为子任务分发给 workers。
/// - Gateway：收到后路由给下级 Agent。
#[derive(Debug, Clone)]
pub struct AgentTask {
    /// 唯一任务 ID。
    pub task_id: String,
    /// 任务指令文本（自然语言描述）。
    pub instruction: String,
    /// 可选的发送方 Agent ID（用于结果回传）。
    pub sender: Option<AgentId>,
    /// 可选的附加上下文（JSON 格式的结构化数据）。
    pub context: Option<serde_json::Value>,
}

/// Agent 执行任务后的返回结果。
#[derive(Debug, Clone)]
pub struct AgentTaskResult {
    /// 对应的任务 ID。
    pub task_id: String,
    /// 任务是否成功完成。
    pub success: bool,
    /// 结果摘要 / 输出文本。
    pub output: String,
    /// 可选的结构化结果数据。
    pub data: Option<serde_json::Value>,
}

/// AgentNode Trait — 多 Agent 拓扑的核心抽象。
///
/// 万物皆 Agent：LLM Worker、AI Pod、Gateway 都是 `AgentNode` 的实现。
/// 核心区别只在于 `assign()` 的行为：
/// - `LlmWorker` — 调用 LLM API 推理
/// - `GatewayNode` — 通过 AccessPoint 路由给外部输入源（人类或远端 Agent）
/// - `Pod` — Master 拆解后分发给子节点
#[async_trait]
pub trait AgentNode: Send + Sync {
    /// 返回此节点的唯一 ID。
    fn id(&self) -> &AgentId;

    /// 查询当前运行状态。
    async fn status(&self) -> AgentStatus;

    /// 该 Agent 的【附加】接入点列表（不含隐式默认接入点）。
    ///
    /// 默认接入点规则（无需配置，框架自动处理）：
    ///   - 非 Root Agent：父节点即默认接入点，通过进程内 MPSC 传输。
    ///   - Root Agent：无父节点，事件只送达此列表中配置的接入点。
    fn access_points(&self) -> &[AccessPoint];

    /// 初始化 Agent（连接 LLM 端点、启动 TUI / gRPC 服务器、加载 Skill 等）。
    async fn init(&self) -> anyhow::Result<()>;

    /// 分配任务给此 Agent。
    ///
    /// - LlmWorker：直接进入 LLM 推理循环。
    /// - GatewayNode：通过 AccessPoint 接收外部输入，路由给下级。
    /// - Pod：Master 拆解后分发给子节点。
    async fn assign(&self, task: AgentTask) -> anyhow::Result<AgentTaskResult>;

    /// 挂起 Agent（保留状态，暂停处理）。
    async fn suspend(&self, reason: &str) -> anyhow::Result<()>;

    /// 从挂起状态恢复。
    async fn resume(&self) -> anyhow::Result<()>;

    /// 终止 Agent，释放所有资源。
    async fn shutdown(&self) -> anyhow::Result<()>;

    /// 返回子节点 ID 列表。
    /// - LlmWorker：空列表。
    /// - Pod / Gateway：非空。
    fn children(&self) -> Vec<AgentId>;

    /// 该节点是否为 Gateway（无 Engine 的纯接入点节点）。
    ///
    /// 框架用此标记来决定「准入审批」请求应路由到哪个节点。
    /// Gateway 节点拥有审批权，可以持有 `can_approve = true` 的接入点。
    fn is_gateway(&self) -> bool {
        false
    }

    /// 将 `Box<dyn AgentNode>` 转换为 `Box<dyn Any>`，用于安全向下转型。
    ///
    /// runtime 模块需要将根节点 downcast 为 `GatewayNode` 以访问
    /// `take_action_rx()` / `take_children()` 等具体方法。
    fn into_any(self: Box<Self>) -> Box<dyn std::any::Any>;

    /// 获取 action sender 返回一个克隆，如果此 Node 支持
    fn action_sender(&self) -> Option<tokio::sync::mpsc::Sender<crate::engine::events::UserAction>> {
        None
    }

    /// 获取事件广播的订阅 receiver，如果此 Node 支持
    fn subscribe_events(&self) -> Option<tokio::sync::broadcast::Receiver<crate::engine::events::EngineEvent>> {
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
