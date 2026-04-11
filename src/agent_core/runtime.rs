//! 多 Agent 运行时入口。
//!
//! 负责从 `ClusterConfig` 构建完整的 AgentNode 树、初始化所有节点、
//! 启动 Gateway 路由循环，并启动 REPL 作为薄客户端。
//!
//! 这是多 Agent 模式的 top-level 入口，由 `main.rs` 在 `multiagent=true` 时调用。

use crate::agent_core::agent::AgentNode;
use crate::agent_core::gateway::GatewayNode;
use crate::agent_core::master::build_agent_tree;
use crate::config::AppConfig;
use crate::control::topology::ClusterConfig;
use crate::permissions::PermissionMode;

/// 多 Agent 模式的完整启动入口。
///
/// ## 执行流程
///
/// ```text
///   1. build_agent_tree() ─► Gateway(子节点已注入)
///   2. gateway.init()     ─► 递归 init 子节点 → 启动接入点
///   3. spawn_backend()    ─► 独立 task: 根据 backend 类型路由到子节点或代理到远端
///   4. run_foreground()   ─► 主线程: REPL / TUI / Ctrl+C 阻塞
///   5. gateway.shutdown() ─► 前台退出后清理 backend + 子节点 + 接入点
/// ```
pub async fn run_multiagent(
    config: AppConfig,
    cluster: &ClusterConfig,
    permission_mode: PermissionMode,
    initial_prompt: Option<String>,
) -> anyhow::Result<()> {
    // ── 1. Build agent tree ───────────────────────────────────────────────
    let root = build_agent_tree(cluster, &config, permission_mode)?;

    // Downcast to GatewayNode — build_agent_tree always returns a Gateway as root.
    let mut gateway: Box<GatewayNode> = root
        .into_any()
        .downcast::<GatewayNode>()
        .map_err(|_| anyhow::anyhow!("Root agent must be kind = \"Gateway\""))?;

    // ── 2. Print topology ─────────────────────────────────────────────────
    eprintln!("  🚀 Multi-Agent Runtime Starting");
    eprintln!("     Gateway: {}", gateway.id());
    for child_id in gateway.children() {
        eprintln!("       └─ {}", child_id);
    }

    // ── 3. Init all nodes (recursive: children first, then gateway) ──────
    gateway.init().await?;
    eprintln!("     ✓ All nodes initialized");

    // ── 4. Spawn backend loop ────────────────────────────────────────────
    let backend_handle = gateway.spawn_backend().await;
    eprintln!("     ✓ Backend loop started");
    eprintln!();

    // ── 5. Run foreground access point (REPL / TUI / Block) ──────────────
    gateway.run_foreground(&config, initial_prompt).await?;

    // ── 6. Shutdown gateway (access points) ──────────────────────────────
    let returned_children = match backend_handle.await {
        Ok(children) => children,
        Err(e) => {
            tracing::error!(error = %e, "Backend loop task panicked");
            Vec::new() // children already lost, but we can still shutdown access points
        }
    };
    gateway.set_children(returned_children);
    gateway.shutdown().await?;
    tracing::info!("Multi-agent runtime shut down");

    Ok(())
}
