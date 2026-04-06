use clap::{Parser, Subcommand};

/// KeZen — A fast, modern AI coding CLI
#[derive(Parser, Debug)]
#[command(
    name = "kezen",
    version,
    about = "A fast, modern AI coding CLI",
    long_about = "KeZen is a blazing-fast AI coding assistant built in Rust.\nIt provides an interactive terminal interface for AI-assisted development."
)]
pub struct Cli {
    /// Send a single prompt (non-interactive mode)
    #[arg(short, long, global = true)]
    pub prompt: Option<String>,

    /// Enable verbose/debug output
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// Bypass permission checks for tool execution
    #[arg(short = 'y', long = "yes", global = true)]
    pub yes: bool,

    /// Override model
    #[arg(long, global = true)]
    pub model: Option<String>,

    /// Override provider (anthropic/openai)
    #[arg(long, global = true)]
    pub provider: Option<String>,

    /// Override API key
    #[arg(long, global = true)]
    pub api_key: Option<String>,

    /// Override max tokens
    #[arg(long, global = true)]
    pub max_tokens: Option<u32>,

    /// Use classic text REPL instead of TUI
    #[arg(long, global = true)]
    pub classic: bool,

    /// Disable MCP server connections
    #[arg(long, global = true)]
    pub no_mcp: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Start an interactive chat session (default)
    Chat {
        /// Send a single prompt (non-interactive)
        #[arg(short, long)]
        prompt: Option<String>,
    },

    /// Start gRPC server
    ServeGrpc {
        /// Server bind address string (e.g. 127.0.0.1:50051)
        #[arg(long, default_value = "127.0.0.1:50051")]
        addr: String,
    },

    /// Initialize project config
    Init,

    /// View/edit configuration
    Config {
        key: Option<String>,
        #[arg(short, long)]
        set: Option<String>,
    },
}
