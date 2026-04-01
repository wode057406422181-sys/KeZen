use clap::{Parser, Subcommand};

/// Infini — A fast, modern AI coding CLI
#[derive(Parser, Debug)]
#[command(
    name = "infini",
    version,
    about = "A fast, modern AI coding CLI",
    long_about = "Infini is a blazing-fast AI coding assistant built in Rust.\nIt provides an interactive terminal interface for AI-assisted development."
)]
pub struct Cli {
    /// Enable verbose/debug output
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// Path to configuration file
    #[arg(short, long, global = true)]
    pub config: Option<String>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Start an interactive coding session
    Chat {
        /// Initial prompt to send
        #[arg(short, long)]
        prompt: Option<String>,
    },

    /// Initialize Infini configuration in the current project
    Init,

    /// Show current configuration
    Config {
        /// Configuration key to get/set
        key: Option<String>,

        /// Value to set
        #[arg(short, long)]
        set: Option<String>,
    },
}
