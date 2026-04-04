pub mod render;
pub mod repl;

use tokio::sync::mpsc;

use crate::config::AppConfig;
use crate::engine::KezenEngine;
use crate::engine::events::{EngineEvent, UserAction};

use crate::tools::registry::create_default_registry;

/// Launch the CLI frontend: spawn the Engine and run the REPL.
pub async fn run_cli(
    config: AppConfig,
    prompt: Option<String>,
    permission_mode: crate::permissions::PermissionMode,
) -> anyhow::Result<()> {
    let (action_tx, action_rx) = mpsc::channel::<UserAction>(32);
    let (event_tx, event_rx) = mpsc::channel::<EngineEvent>(32);

    let registry = create_default_registry(&config);
    let engine = KezenEngine::new(config.clone(), action_rx, event_tx, registry, permission_mode).await?;

    // Spawn the engine loop in a background task
    tokio::spawn(async move {
        engine.run().await;
    });

    // Run the REPL on the main task (blocking readline)
    repl::run_repl(config, action_tx, event_rx, prompt).await
}
