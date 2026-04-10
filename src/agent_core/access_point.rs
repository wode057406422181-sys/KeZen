use std::fmt;
use std::net::SocketAddr;

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
    Grpc {
        addr: SocketAddr,
        can_approve: bool,
    },

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
}
