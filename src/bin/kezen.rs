use anyhow::Result;
use clap::Parser;
use kezen::*;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

use kezen::cli::{Cli, Command, KeysCommand};
use kezen::config::Provider;
use secrecy::SecretString;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize tracing — file-only.
    // All operational logs go to ~/.kezen/logs/kezen.log (daily rolling).
    // No stderr layer: it would corrupt TUI rendering and interleave with REPL output.
    // For startup diagnostics, use eprintln! directly (before TUI/REPL takes over).
    let kezen_home = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?
        .join(".kezen");
    let log_dir = kezen_home.join("logs");

    // Validate log directories are writable before anything else.
    // This catches permission issues, full disks, etc. early.
    for (label, dir) in [
        ("logs", kezen_home.join("logs")),
        ("sessions", kezen_home.join("sessions")),
        ("api_logs", kezen_home.join("api_logs")),
    ] {
        if let Err(e) = tokio::fs::create_dir_all(&dir).await {
            anyhow::bail!("Cannot create {} dir ({}): {}", label, dir.display(), e);
        }
        // Probe writability with a temp file
        let probe = dir.join(".write_probe");
        match tokio::fs::write(&probe, b"ok").await {
            Ok(_) => {
                let _ = tokio::fs::remove_file(&probe).await;
            }
            Err(e) => {
                anyhow::bail!("{} dir is not writable ({}): {}", label, dir.display(), e);
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

    // ── Handle generic utility commands first ───────────────────────
    // These commands do not need full engine configuration or API key resolution.
    if let Some(Command::Keys { ref command }) = cli.command {
        match command {
            KeysCommand::Set { profile, key } => {
                kezen::config::keys::set_key(profile, key)?;
                println!(
                    "Successfully stored API key for profile '{}' in secure credentials file.",
                    profile
                );

                // For keys set, we explicitly load JUST the model profiles to avoid full engine crash
                // on missing API keys that we are currently resolving!
                let mut models_config =
                    kezen::config::model::ModelsConfig::load().unwrap_or_default();
                if !models_config.models.contains_key(profile) {
                    models_config.models.insert(
                        profile.clone(),
                        kezen::config::ModelProfile {
                            provider: kezen::config::Provider::Anthropic,
                            model: format!("{}-model", profile),
                            api_key: Some(SecretString::from(format!("keystore://{}", profile))),
                            ..Default::default()
                        },
                    );
                } else if let Some(p) = models_config.models.get_mut(profile) {
                    p.api_key = Some(SecretString::from(format!("keystore://{}", profile)));
                }
                models_config.save()?;
                println!(
                    "Updated ~/.kezen/config/model.toml to use keystore://{}",
                    profile
                );
            }
        }
        return Ok(());
    }

    // Load config (file + env vars)
    let mut config = config::AppConfig::load()?;

    let permission_mode = if cli.yes {
        kezen::permissions::PermissionMode::DontAsk
    } else {
        kezen::permissions::PermissionMode::Default
    };

    // Layer 1: CLI argument overrides (highest priority)
    if let Some(ref m) = cli.model {
        config.resolve_model_profile(m);
    } else if let Some(m) = config.model.clone() {
        config.resolve_model_profile(&m);
    }
    if let Some(ref p) = cli.provider {
        config.runtime_profile.provider = match p.to_lowercase().as_str() {
            "openai" => Provider::OpenAi,
            _ => Provider::Anthropic,
        };
    }
    if let Some(ref k) = cli.api_key {
        config.runtime_profile.api_key = kezen::config::keys::resolve_key(Some(k.clone()));
    }
    if cli.no_mcp {
        config.no_mcp = true;
    }

    if config.multiagent {
        if let Ok(config_file) = config::AppConfig::config_path() {
            match kezen::control::topology::load_cluster_config(&config_file).await {
                Ok(cluster) => {
                    eprintln!("  🚀 Multi-Agent Mode Detected!");
                    eprintln!(
                        "     Cluster: {}",
                        cluster.cluster.name.as_deref().unwrap_or("unnamed")
                    );
                    eprintln!("     Agents : {}", cluster.agents.len());
                    if let Some(wd) = &cluster.cluster.work_dir {
                        eprintln!("     WorkDir: {}", wd.display());
                    }

                    let gateway = cluster
                        .agents
                        .iter()
                        .find(|a| a.kind.as_ref() == Some(&kezen::control::topology::AgentKind::Gateway))
                        .ok_or_else(|| anyhow::anyhow!("CRITICAL ERROR: No agent with 'kind = \"Gateway\"' defined in kezen.toml. Multi-agent mode requires a Gateway node."))?;

                    config.merge_with_toml(gateway, &cluster);

                    return kezen::agent_core::runtime::run_multiagent(
                        config,
                        &cluster,
                        permission_mode,
                        cli.prompt,
                    )
                    .await;
                }
                Err(e) => {
                    anyhow::bail!(
                        "Failed to load cluster topology from {}: {}",
                        config_file.display(),
                        e
                    );
                }
            }
        }
    }

    // Enable API debug logging if --verbose
    if cli.verbose {
        api::debug_logger::enable_debug_logging();
        eprintln!("  🔍 API debug logging enabled → ~/.kezen/api_logs/");
    }

    // Clean up audit logs older than 30 days
    audit::cleanup_old_audit_logs().await;

    match cli.command {
        Some(Command::ServeGrpc { addr }) => {
            let socket_addr = addr
                .parse()
                .map_err(|e| anyhow::anyhow!("Invalid address {}: {}", addr, e))?;
            let (action_tx, action_rx) =
                tokio::sync::mpsc::channel(kezen::constants::engine::ACTION_CHANNEL_BUFFER);
            let (event_tx, _) =
                tokio::sync::broadcast::channel(kezen::constants::engine::EVENT_CHANNEL_BUFFER);

            let work_dir = std::env::current_dir()?;
            let registry =
                kezen::tools::registry::create_default_registry(&config, work_dir.clone());
            let engine = engine::KezenEngine::new(
                config.clone(),
                action_rx,
                event_tx.clone(),
                registry,
                permission_mode,
                work_dir,
            )
            .await?;

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
        Some(Command::Keys { .. }) => {
            unreachable!("Keys command handled early");
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
