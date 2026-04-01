mod chat;

use anyhow::Result;

use crate::cli::{Cli, Command};

pub async fn execute(cli: Cli) -> Result<()> {
    match cli.command {
        Some(Command::Chat { prompt }) => chat::run(prompt).await,
        Some(Command::Init) => {
            println!("Initializing Infini in current directory...");
            // TODO: create .infini/ config directory
            Ok(())
        }
        Some(Command::Config { key, set }) => {
            match (key, set) {
                (Some(k), Some(v)) => println!("Setting {k} = {v}"),
                (Some(k), None) => println!("Getting config: {k}"),
                _ => println!("Current configuration: (default)"),
            }
            Ok(())
        }
        None => {
            // Default: start interactive chat session
            chat::run(None).await
        }
    }
}
