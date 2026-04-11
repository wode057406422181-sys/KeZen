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
use crate::engine::events::{EngineEvent, UserAction};
use crate::permissions::PermissionMode;

/// 多 Agent 模式的完整启动入口。
///
/// ## 执行流程
///
/// ```text
///   1. build_agent_tree() ─► Gateway(子节点已注入)
///   2. gateway.init()     ─► 递归 init 子节点 → 启动接入点
///   3. take_action_rx()   ─► 取走 action receiver
///   4. take_children()    ─► 取走子节点句柄
///   5. spawn routing_loop ─► 独立 task: action_rx → 路由到子节点 → 转发事件
///   6. run_repl()         ─► 主线程: 复用现有 REPL 作为薄客户端
///   7. gateway.shutdown() ─► REPL 退出后清理
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
    let gateway: Box<GatewayNode> = root
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

    // ── 4. Grab channel handles & children ───────────────────────────────
    let action_tx = gateway.action_sender();
    let event_tx = gateway.event_sender();
    let event_rx = event_tx.subscribe();
    let mut action_rx = gateway.take_action_rx().await;
    let mut children = gateway.take_children().await;

    // ── 5. Spawn routing loop ────────────────────────────────────────────
    let gateway_id = gateway.id().clone();
    let event_tx_for_loop = event_tx.clone();

    let routing_handle = tokio::spawn(async move {
        tracing::info!(agent = %gateway_id, "Routing loop started");

        if children.is_empty() {
            tracing::error!(agent = %gateway_id, "No children — routing loop exiting");
            return;
        }

        let mut task_counter = 0u64;
        let mut pending_actions: std::collections::VecDeque<UserAction> = std::collections::VecDeque::new();

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
                        // 支持流式：直接将指令发送给子节点的 channel
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

                        // 循环读取子节点抛出的事件，同时监听并转发新的动作（如 Cancel）
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
                        // 不支持流式的降级方案：使用 assign() 阻塞等待完毕
                        let task = crate::agent_core::agent::AgentTask {
                            task_id: task_id.clone(),
                            instruction: content,
                            sender: Some(gateway_id.clone()),
                            context: None,
                        };
                        match child.node.assign(task).await {
                            Ok(result) => {
                                if !result.output.is_empty() {
                                    let _ = event_tx_for_loop.send(EngineEvent::TextDelta {
                                        text: result.output,
                                    });
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

        // Shutdown all children.
        for child in children.iter_mut() {
            if let Err(e) = child.node.shutdown().await {
                tracing::warn!(child = %child.node.id(), error = %e, "Child shutdown error");
            }
        }
    });

    // ── 6. Run REPL as thin client ───────────────────────────────────────
    eprintln!("     ✓ Routing loop started");
    eprintln!();

    crate::frontend::repl::repl::run_repl(config, action_tx, event_rx, initial_prompt).await?;

    // ── 7. Shutdown gateway (access points) ──────────────────────────────
    let _ = routing_handle.await;
    gateway.shutdown().await?;
    tracing::info!("Multi-agent runtime shut down");

    Ok(())
}
