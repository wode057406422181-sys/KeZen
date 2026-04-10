use anyhow::Result;
use clap::Parser;

use kezen::config::AppConfig;
use kezen::frontend::grpc::client::run_grpc_client;

#[derive(Parser)]
#[command(name = "kezen-client", about = "KeZen Thin Client (gRPC only)")]
struct ClientCli {
    /// gRPC server URL
    #[arg(short, long, default_value = "http://127.0.0.1:50051")]
    url: String,

    /// Optional initial prompt
    #[arg(short, long)]
    prompt: Option<String>,

    /// Start in TUI mode
    #[arg(long)]
    tui: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = ClientCli::parse();

    // The client doesn't need full AppConfig, just enough for the REPL/TUI to render.
    // The server handles the actual model execution.
    let config = AppConfig::default();

    let (action_tx, action_rx) = tokio::sync::mpsc::channel(32);
    let (event_tx, event_rx) = tokio::sync::broadcast::channel(64);

    let url_clone = cli.url.clone();
    let event_tx_clone = event_tx.clone();

    // Spawn the gRPC client adapter task
    tokio::spawn(async move {
        if let Err(e) = run_grpc_client(url_clone, action_rx, event_tx_clone).await {
            tracing::error!("gRPC client error: {}", e);
            eprintln!("Lost connection to server: {}", e);
        }
    });

    if cli.tui {
        use crossterm::{
            execute,
            terminal::{
                EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
            },
        };
        use ratatui::{Terminal, backend::CrosstermBackend};
        use std::io;

        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;
        terminal.clear()?;

        let result = kezen::frontend::tui::app::run_app(
            &mut terminal,
            config,
            action_tx,
            event_rx,
            cli.prompt,
        )
        .await;

        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
        terminal.show_cursor()?;

        result
    } else {
        kezen::frontend::repl::repl::run_repl(config, action_tx, event_rx, cli.prompt).await
    }
}
