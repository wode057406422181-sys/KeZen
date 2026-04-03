mod api;
mod cli;
mod cli_frontend;
mod config;
mod constants;
mod engine;
mod error;
mod prompts;
mod server;

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, Command};
use crate::config::Provider;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing/logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

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

    // Enable API debug logging if --verbose
    if cli.verbose {
        api::debug_logger::enable_debug_logging();
        eprintln!("  🔍 API debug logging enabled → ~/.infini/logs/");
    }

    match cli.command {
        Some(Command::Serve { port, host }) => server::run_server(config, host, port).await,
        Some(Command::Chat { prompt }) => {
            // Chat subcommand: use its --prompt or fall back to top-level --prompt
            let effective_prompt = prompt.or(cli.prompt);
            cli_frontend::run_cli(config, effective_prompt).await
        }
        Some(Command::Init) => {
            println!("Initializing Infini in current directory...");
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
            // Default: if --prompt is given, single-shot; otherwise interactive
            cli_frontend::run_cli(config, cli.prompt).await
        }
    }
}
pub mod tools;
