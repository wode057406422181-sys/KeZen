use std::fmt;
use std::net::SocketAddr;
use tokio::sync::{broadcast, mpsc};

use crate::engine::events::{EngineEvent, UserAction};

/// Agent 的附加接入点类型。
///
/// 每个 Agent 有且仅有一个隐式默认接入点（父节点 MPSC 通道），
/// 此枚举定义的是额外挂载的接入点。
///
/// 引擎产生的每一条 Event 会写入：
///   [父节点 MPSC（若有）] + [此处列出的所有附加接入点]
#[derive(Debug, Clone, PartialEq)]
pub enum AccessPoint {
    /// 本地终端 TUI（ratatui）。
    Tui { can_approve: bool },

    /// 经典文本 REPL（rustyline），轻量级替代 TUI。
    Repl { can_approve: bool },

    /// gRPC 双向流接入点。
    Grpc { addr: SocketAddr, can_approve: bool },

    /// 进程内 MPSC 通道（无网络开销）。
    /// 用于父子 Agent 之间的进程内通信。
    InProcess,
}

impl AccessPoint {
    /// Whether this access point has approval authority.
    pub fn can_approve(&self) -> bool {
        match self {
            Self::Tui { can_approve } => *can_approve,
            Self::Repl { can_approve } => *can_approve,
            Self::Grpc { can_approve, .. } => *can_approve,
            Self::InProcess => false,
        }
    }

    /// A short type label for logging/display.
    pub fn kind_label(&self) -> &'static str {
        match self {
            Self::Tui { .. } => "TUI",
            Self::Repl { .. } => "REPL",
            Self::Grpc { .. } => "gRPC",
            Self::InProcess => "InProcess",
        }
    }
}

impl fmt::Display for AccessPoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tui { can_approve } => write!(f, "TUI(approve={})", can_approve),
            Self::Repl { can_approve } => write!(f, "REPL(approve={})", can_approve),
            Self::Grpc { addr, can_approve } => {
                write!(f, "gRPC({}; approve={})", addr, can_approve)
            }
            Self::InProcess => write!(f, "InProcess"),
        }
    }
}

/// 接入点启动后的运行时句柄。
///
/// 持有：
///   - 接入点配置副本（用于查询 `can_approve` 等属性）
///   - 该接入点的 `broadcast::Receiver<EngineEvent>`（自动由 `event_tx.subscribe()` 创建）
///   - spawned task 的 `JoinHandle`，用于生命周期管理
pub struct AccessPointHandle {
    /// 接入点配置信息（类型 + 属性）。
    pub config: AccessPoint,
    /// 该接入点的 tokio task handle。
    /// `None` 如果接入点尚未启动或已关闭。
    pub task_handle: Option<tokio::task::JoinHandle<()>>,
}

impl AccessPointHandle {
    /// 检查该接入点的 task 是否已终止。
    pub fn is_finished(&self) -> bool {
        self.task_handle.as_ref().map_or(true, |h| h.is_finished())
    }

    /// 终止该接入点。
    pub fn abort(&mut self) {
        if let Some(h) = self.task_handle.take() {
            h.abort();
        }
    }
}

/// 启动一个接入点。
///
/// 根据 `AccessPoint` 类型启动对应的前端服务（gRPC 服务器等），
/// 每个接入点获得：
///   - `event_rx`: 从 Gateway 的 event broadcast 订阅而来
///   - `action_tx`: 共享的 MPSC sender，所有接入点的 UserAction 汇入同一个通道
///
/// # Returns
/// 一个 `AccessPointHandle`，持有 spawn 出的 task handle。
///
/// # Note
/// TUI 和 REPL 接入点当前仅记录日志（它们需要终端独占，不适合作为 Gateway 的被动接入点）。
/// gRPC 接入点会真正启动 tonic 服务器。
pub async fn start_access_point(
    ap: &AccessPoint,
    action_tx: mpsc::Sender<UserAction>,
    event_tx: broadcast::Sender<EngineEvent>,
) -> anyhow::Result<AccessPointHandle> {
    match ap {
        AccessPoint::Grpc { addr, can_approve } => {
            let addr = *addr;
            let can_approve = *can_approve;
            tracing::info!(%addr, can_approve, "Starting gRPC access point");

            let handle = tokio::spawn(async move {
                if let Err(e) =
                    crate::frontend::grpc::start_grpc_server(addr, action_tx, event_tx).await
                {
                    tracing::error!(%addr, error = %e, "gRPC access point failed");
                }
            });

            Ok(AccessPointHandle {
                config: ap.clone(),
                task_handle: Some(handle),
            })
        }
        AccessPoint::Tui { can_approve } | AccessPoint::Repl { can_approve } => {
            // 前台接入点（TUI / REPL）由 GatewayNode::run_foreground() 管理，此处跳过启动逻辑。
            tracing::info!(
                can_approve,
                kind = ap.kind_label(),
                "Foreground access point registered (handled by run_foreground)"
            );
            Ok(AccessPointHandle {
                config: ap.clone(),
                task_handle: None,
            })
        }
        AccessPoint::InProcess => {
            // 进程内通道不需要额外启动任何服务。
            Ok(AccessPointHandle {
                config: ap.clone(),
                task_handle: None,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tui_access_point_can_approve() {
        let ap = AccessPoint::Tui { can_approve: true };
        assert!(ap.can_approve());
    }

    #[test]
    fn repl_access_point_cannot_approve() {
        let ap = AccessPoint::Repl { can_approve: false };
        assert!(!ap.can_approve());
    }

    #[test]
    fn grpc_access_point_with_addr() {
        let ap = AccessPoint::Grpc {
            addr: "127.0.0.1:50052".parse().unwrap(),
            can_approve: true,
        };
        assert!(ap.can_approve());
        assert!(format!("{}", ap).contains("127.0.0.1:50052"));
    }

    #[test]
    fn in_process_cannot_approve() {
        let ap = AccessPoint::InProcess;
        assert!(!ap.can_approve());
    }

    #[test]
    fn display_formats_correctly() {
        let tui = AccessPoint::Tui { can_approve: true };
        assert_eq!(format!("{}", tui), "TUI(approve=true)");

        let repl = AccessPoint::Repl { can_approve: false };
        assert_eq!(format!("{}", repl), "REPL(approve=false)");
    }

    #[test]
    fn kind_label() {
        assert_eq!(AccessPoint::Tui { can_approve: true }.kind_label(), "TUI");
        assert_eq!(
            AccessPoint::Repl { can_approve: false }.kind_label(),
            "REPL"
        );
        assert_eq!(
            AccessPoint::Grpc {
                addr: "127.0.0.1:50052".parse().unwrap(),
                can_approve: true
            }
            .kind_label(),
            "gRPC"
        );
        assert_eq!(AccessPoint::InProcess.kind_label(), "InProcess");
    }

    #[test]
    fn access_point_handle_is_finished_when_no_task() {
        let handle = AccessPointHandle {
            config: AccessPoint::InProcess,
            task_handle: None,
        };
        assert!(handle.is_finished());
    }

    #[tokio::test]
    async fn access_point_handle_abort() {
        let task = tokio::spawn(async {
            // Long-running task
            tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
        });
        let mut handle = AccessPointHandle {
            config: AccessPoint::InProcess,
            task_handle: Some(task),
        };
        assert!(!handle.is_finished());
        handle.abort();
        assert!(handle.task_handle.is_none());
    }

    #[tokio::test]
    async fn start_tui_returns_passive_handle() {
        let (action_tx, _action_rx) = mpsc::channel(32);
        let (event_tx, _) = broadcast::channel(64);

        let ap = AccessPoint::Tui { can_approve: true };
        let handle = start_access_point(&ap, action_tx, event_tx).await.unwrap();

        assert!(handle.task_handle.is_none()); // passive
        assert!(handle.config.can_approve());
    }

    #[tokio::test]
    async fn start_repl_returns_passive_handle() {
        let (action_tx, _action_rx) = mpsc::channel(32);
        let (event_tx, _) = broadcast::channel(64);

        let ap = AccessPoint::Repl { can_approve: false };
        let handle = start_access_point(&ap, action_tx, event_tx).await.unwrap();

        assert!(handle.task_handle.is_none());
        assert!(!handle.config.can_approve());
    }

    #[tokio::test]
    async fn start_in_process_returns_passive_handle() {
        let (action_tx, _action_rx) = mpsc::channel(32);
        let (event_tx, _) = broadcast::channel(64);

        let ap = AccessPoint::InProcess;
        let handle = start_access_point(&ap, action_tx, event_tx).await.unwrap();

        assert!(handle.task_handle.is_none());
    }

    #[tokio::test]
    async fn can_approve_propagates_through_handle() {
        let (action_tx, _action_rx) = mpsc::channel(32);
        let (event_tx, _) = broadcast::channel(64);

        let ap = AccessPoint::Grpc {
            addr: "127.0.0.1:0".parse().unwrap(),
            can_approve: true,
        };
        // Note: gRPC server start will likely fail to bind on port 0 in some envs,
        // but the handle is created before the server runs, so we test the config.
        let handle = start_access_point(&ap, action_tx, event_tx).await.unwrap();
        assert!(handle.config.can_approve());
        assert_eq!(handle.config.kind_label(), "gRPC");
    }

    #[tokio::test]
    async fn multiple_access_points_share_event_broadcast() {
        let (event_tx, _) = broadcast::channel::<EngineEvent>(64);

        // Simulate multiple access points subscribing
        let mut rx1 = event_tx.subscribe();
        let mut rx2 = event_tx.subscribe();
        let mut rx3 = event_tx.subscribe();

        // Gateway sends an event
        let _ = event_tx.send(EngineEvent::TextDelta {
            text: "broadcast_test".to_string(),
        });

        // All access points should receive it
        let e1 = rx1.recv().await.unwrap();
        let e2 = rx2.recv().await.unwrap();
        let e3 = rx3.recv().await.unwrap();

        assert!(matches!(e1, EngineEvent::TextDelta { text } if text == "broadcast_test"));
        assert!(matches!(e2, EngineEvent::TextDelta { text } if text == "broadcast_test"));
        assert!(matches!(e3, EngineEvent::TextDelta { text } if text == "broadcast_test"));
    }

    #[tokio::test]
    async fn multiple_access_points_merge_actions() {
        let (action_tx, mut action_rx) = mpsc::channel::<UserAction>(64);

        // Simulate 3 access points all holding clones of action_tx
        let tx1 = action_tx.clone();
        let tx2 = action_tx.clone();
        let tx3 = action_tx;

        tx1.send(UserAction::SendMessage {
            content: "from_tui".to_string(),
        })
        .await
        .unwrap();
        tx2.send(UserAction::SendMessage {
            content: "from_repl".to_string(),
        })
        .await
        .unwrap();
        tx3.send(UserAction::SendMessage {
            content: "from_grpc".to_string(),
        })
        .await
        .unwrap();

        // Gateway receives all 3 merged into a single stream
        let a1 = action_rx.recv().await.unwrap();
        let a2 = action_rx.recv().await.unwrap();
        let a3 = action_rx.recv().await.unwrap();

        assert_eq!(
            a1,
            UserAction::SendMessage {
                content: "from_tui".to_string()
            }
        );
        assert_eq!(
            a2,
            UserAction::SendMessage {
                content: "from_repl".to_string()
            }
        );
        assert_eq!(
            a3,
            UserAction::SendMessage {
                content: "from_grpc".to_string()
            }
        );
    }
}
