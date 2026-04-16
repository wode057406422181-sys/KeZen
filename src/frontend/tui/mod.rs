pub mod app;
pub mod ui;

use std::io;

use crossterm::{
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::{broadcast, mpsc};

use crate::config::AppConfig;
use crate::constants::engine::{ACTION_CHANNEL_BUFFER, EVENT_CHANNEL_BUFFER};
use crate::engine::KezenEngine;
use crate::engine::events::{EngineEvent, UserAction};
use crate::tools::registry::create_default_registry;

/// Launch the full-screen TUI frontend.
///
/// Creates an Engine (identical to `crate::frontend::repl::run_cli`),
/// initialises crossterm raw mode + alternate screen,
/// runs the ratatui main loop, then restores the terminal on exit.
pub async fn run_tui(
    config: AppConfig,
    prompt: Option<String>,
    permission_mode: crate::permissions::PermissionMode,
) -> anyhow::Result<()> {
    // ── 1. Engine channels ──────────────────────────────────────────────
    let (action_tx, action_rx) = mpsc::channel::<UserAction>(ACTION_CHANNEL_BUFFER);
    let (event_tx, event_rx) = broadcast::channel::<EngineEvent>(EVENT_CHANNEL_BUFFER);

    // ── 2. Start Engine in background ──────────────────────────────────
    let work_dir = std::env::current_dir()?;
    let registry = create_default_registry(&config, work_dir.clone());
    let engine = KezenEngine::new(
        config.clone(),
        action_rx,
        event_tx,
        registry,
        permission_mode,
        work_dir,
    )
    .await?;
    tokio::spawn(async move {
        engine.run().await;
    });

    // ── 3. Initialise terminal ─────────────────────────────────────────
    run_tui_client(config, action_tx, event_rx, prompt).await
}

struct TerminalRestoreGuard;
impl Drop for TerminalRestoreGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen);
    }
}

/// Start the TUI client loop interacting via provided channels.
/// This acts as a downstream client for an already running Engine or Gateway.
pub async fn run_tui_client(
    config: AppConfig,
    action_tx: mpsc::Sender<UserAction>,
    event_rx: broadcast::Receiver<EngineEvent>,
    prompt: Option<String>,
) -> anyhow::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    // Use a custom Drop guard to ensure terminal is restored even if app panics
    let _guard = TerminalRestoreGuard;

    // ── 4. Run TUI main loop ───────────────────────────────────────────
    let result = app::run_app(&mut terminal, config, action_tx, event_rx, prompt).await;

    // 5. Normal terminal restore is handled by the scopeguard. We can just explicitly show cursor.
    terminal.show_cursor()?;

    result
}
