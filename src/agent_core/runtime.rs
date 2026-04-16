//! Multi-Agent execution entry point.
//!
//! Responsible for building the full AgentNode tree from `ClusterConfig`, initializing all nodes,
//! starting the Gateway routing loop, and launching the REPL as a thin client.
//!
//! This is the top-level entry point for Multi-Agent mode, called by `main.rs` when `multiagent=true`.

use crate::agent_core::agent::AgentNode;
use crate::agent_core::gateway::GatewayNode;
use crate::agent_core::master::build_agent_tree;
use crate::config::AppConfig;
use crate::control::topology::ClusterConfig;
use crate::permissions::PermissionMode;

/// The complete startup entry point for Multi-Agent mode.
///
/// ## Execution Flow
///
/// ```text
///   1. build_agent_tree() ─► Gateway(Child nodes injected)
///   2. gateway.init()     ─► Recursively init child nodes → Start access points
///   3. spawn_backend()    ─► Background task: Route to child nodes or proxy to remote depending on backend type
///   4. run_foreground()   ─► Main thread: REPL / TUI / Ctrl+C blocking
///   5. gateway.shutdown() ─► Clean up backend + child nodes + access points upon foreground exit
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
