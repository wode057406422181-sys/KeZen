mod api;
mod audit;
mod cli;
mod config;
mod constants;
mod context;
mod control;
mod cost;
mod engine;
mod error;
mod frontend;
mod mcp;
mod permissions;
mod prompts;
mod session;
mod skills;
pub mod tools;

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

use crate::cli::{Cli, Command};
use crate::config::Provider;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize tracing — file-only.
    // All operational logs go to ~/.kezen/logs/kezen.log (daily rolling).
    // No stderr layer: it would corrupt TUI rendering and interleave with REPL output.
    // For startup diagnostics, use eprintln! directly (before TUI/REPL takes over).
    let kezen_home = dirs::home_dir()
        .expect("Cannot determine home directory")
        .join(".kezen");
    let log_dir = kezen_home.join("logs");

    // Validate log directories are writable before anything else.
    // This catches permission issues, full disks, etc. early — if we can't write
    // logs, we warn the user while we still have access to stderr (before TUI/REPL).
    for (label, dir) in [
        ("logs", kezen_home.join("logs")),
        ("sessions", kezen_home.join("sessions")),
        ("api_logs", kezen_home.join("api_logs")),
    ] {
        if let Err(e) = std::fs::create_dir_all(&dir) {
            eprintln!("  ⚠ Cannot create {} dir ({}): {}", label, dir.display(), e);
            continue;
        }
        // Probe writability with a temp file
        let probe = dir.join(".write_probe");
        match std::fs::write(&probe, b"ok") {
            Ok(_) => { let _ = std::fs::remove_file(&probe); }
            Err(e) => {
                eprintln!("  ⚠ {} dir is not writable ({}): {}", label, dir.display(), e);
            }
        }
    }

    let file_appender = tracing_appender::rolling::daily(&log_dir, "kezen.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    let file_filter = if cli.verbose {
        EnvFilter::new("debug")
    } else {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("kezen=info"))
    };

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(non_blocking)
                .with_ansi(false)
                .with_filter(file_filter),
        )
        .init();

    // _guard must live until main() returns; dropping it flushes buffered logs.

    // Load config (file + env vars)
    let mut config = config::AppConfig::load()?;

    // Layer 1: CLI argument overrides (highest priority)
    if let Some(ref m) = cli.model {
        config.model = Some(m.clone());
    }
    if let Some(ref p) = cli.provider {
        config.provider = match p.to_lowercase().as_str() {
            "openai" => Provider::OpenAi,
            _ => Provider::Anthropic,
        };
    }
    if let Some(ref k) = cli.api_key {
        config.api_key = Some(k.clone());
    }
    if let Some(t) = cli.max_tokens {
        config.max_tokens = Some(t);
    }
    if cli.no_mcp {
        config.no_mcp = true;
    }

    // Enable API debug logging if --verbose
    if cli.verbose {
        api::debug_logger::enable_debug_logging();
        eprintln!("  🔍 API debug logging enabled → ~/.kezen/api_logs/");
    }

    // Clean up audit logs older than 30 days
    audit::cleanup_old_audit_logs().await;

    let permission_mode = if cli.yes {
        crate::permissions::PermissionMode::DontAsk
    } else {
        crate::permissions::PermissionMode::Default
    };

    match cli.command {
        Some(Command::ServeGrpc { addr }) => {
            let socket_addr = addr.parse().unwrap_or_else(|e| {
                tracing::error!("Invalid address {}: {}", addr, e);
                std::process::exit(1);
            });
            let (action_tx, action_rx) = tokio::sync::mpsc::channel(crate::constants::engine::ACTION_CHANNEL_BUFFER);
            let (event_tx, _) = tokio::sync::broadcast::channel(crate::constants::engine::EVENT_CHANNEL_BUFFER);

            let work_dir = std::env::current_dir()?;
            let registry = crate::tools::registry::create_default_registry(&config, work_dir.clone());
            let engine = engine::KezenEngine::new(config.clone(), action_rx, event_tx.clone(), registry, permission_mode, work_dir).await?;

            tokio::spawn(async move {
                engine.run().await;
            });

            frontend::grpc::start_grpc_server(socket_addr, action_tx, event_tx).await
        }
        Some(Command::Chat { prompt }) => {
            // Chat subcommand: use its --prompt or fall back to top-level --prompt
            let effective_prompt = prompt.or(cli.prompt);
            if cli.tui {
                frontend::tui::run_tui(config, effective_prompt, permission_mode).await
            } else {
                frontend::repl::run_cli(config, effective_prompt, permission_mode).await
            }
        }
        Some(Command::Init) => {
            println!("Initializing KeZen in current directory...");
            config.save()?;
            if let Ok(path) = config::AppConfig::config_path() {
                println!("Configuration saved to {}", path.display());
            } else {
                println!("Configuration saved successfully");
            }
            Ok(())
        }
        Some(Command::Config { key, set }) => {
            match (key, set) {
                (Some(k), Some(v)) => println!("Setting {k} = {v} (not fully implemented)"),
                (Some(k), None) => println!("Getting config: {k} (not fully implemented)"),
                _ => println!("Current configuration:\n{:#?}", config),
            }
            Ok(())
        }
        None => {
            // Default: REPL mode unless --tui is given
            if cli.tui {
                frontend::tui::run_tui(config, cli.prompt, permission_mode).await
            } else {
                frontend::repl::run_cli(config, cli.prompt, permission_mode).await
            }
        }
    }
}
