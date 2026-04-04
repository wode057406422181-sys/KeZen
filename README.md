# ⚡ Infini

A fast, modern AI coding CLI built in Rust — with agentic tool orchestration, multi-provider support, and real-time cost tracking.

## Overview

Infini is a blazing-fast AI coding assistant built from the ground up in Rust. It provides an interactive REPL for AI-assisted development with agentic capabilities — the AI can autonomously read, write, search, and execute commands on your codebase with a permission-gated safety model.

## Features

- 🚀 **Blazing fast** — Native Rust performance with zero-cost abstractions
- 🤖 **Agentic Loop** — AI autonomously calls tools, processes results, and iterates until the task is complete
- 🔌 **Multi-Provider** — Supports Anthropic (Claude), OpenAI (GPT/o-series), Google (Gemini), and OpenAI-compatible endpoints (Qwen, Kimi, GLM, MiniMax, etc.)
- 🛠️ **6 Built-in Tools** — Bash, FileRead, FileWrite, FileEdit, Grep, Glob — all with async parallel execution
- 🔒 **Permission System** — Destructive operations require user approval; use `-y` to bypass
- 🧠 **Context Management** — 3-tier hierarchical memory (`.infini.md`) + Git context injection
- 💾 **Session Persistence** — JSON snapshots with `/resume` to restore previous sessions
- 💰 **Cost Tracking** — Regex-based model pricing with session-level USD cost reporting
- ⚡ **Async-first** — Built on Tokio for efficient streaming I/O
- 🎨 **Rich Terminal UI** — Markdown rendering, syntax highlighting, and thinking indicator

## Getting Started

### Prerequisites

- Rust 1.85+ (install via [rustup](https://rustup.rs/))

### Build & Run

```bash
# Build
cargo build

# Run in REPL mode
cargo run

# Run with environment variables
ANTHROPIC_API_KEY=sk-xxx cargo run

# Show help
cargo run -- --help
```

### Install locally

```bash
cargo install --path .
infini --help
```

### Configuration

Infini loads configuration from `~/.config/infini/config.toml`:

```toml
[default]
provider = "anthropic"          # or "openai"
model = "claude-sonnet-4-6"
api_key = "sk-..."
# base_url = "https://api.anthropic.com"  # optional, for proxies
```

You can also configure via environment variables:

```bash
export ANTHROPIC_API_KEY=sk-xxx
export INFINI_MODEL=claude-sonnet-4-6
```

## REPL Commands

| Command | Description |
|---------|-------------|
| `/help` | Show available commands |
| `/quit` | Exit and display session cost summary |
| `/clear` | Clear conversation history |
| `/resume [id]` | Restore a previous session |
| `/cost` | Show current session cost |

## Project Structure

```
src/
├── main.rs              # Entry point, tracing init, CLI dispatch
├── cli.rs               # CLI argument definitions (clap derive)
├── config.rs            # Configuration loading (TOML + env)
├── cost.rs              # Regex-based model pricing & cost tracking
├── error.rs             # Custom error types (thiserror)
├── api/                 # LLM provider clients
│   ├── anthropic.rs     # Anthropic Messages API (SSE streaming)
│   ├── openai.rs        # OpenAI Chat Completions (SSE streaming)
│   └── types.rs         # Shared types: Message, StreamEvent, Usage
├── engine/              # Core agentic engine
│   ├── mod.rs           # Agentic loop: stream → tool → approval → execute
│   └── session.rs       # Session state (tokens, cost, messages)
├── cli_frontend/        # Terminal frontend
│   └── repl.rs          # REPL loop, slash commands, event rendering
├── server/              # HTTP server (Axum)
│   └── routes.rs        # REST API routes
├── tools/               # Tool implementations
│   ├── mod.rs           # Tool trait definition
│   ├── registry.rs      # Dynamic tool registry
│   ├── bash.rs          # Shell command execution
│   ├── file_read.rs     # File reading with line ranges
│   ├── file_write.rs    # File creation / overwrite
│   ├── file_edit.rs     # Surgical string replacement
│   ├── grep.rs          # Regex search across files
│   └── glob.rs          # File pattern matching
├── context/             # Context management
│   ├── memory.rs        # .infini.md hierarchical memory
│   └── git.rs           # Git context collection
├── permissions/         # Permission gating
│   └── mod.rs           # PermissionState, approval flow
├── session/             # Session persistence
│   └── mod.rs           # JSON snapshot save/load/resume
├── prompts/             # System prompt templates
│   └── mod.rs           # System prompt construction
└── constants/           # Shared constants
    ├── api.rs           # API version strings
    └── defaults.rs      # Default config values
```

## Architecture

```
User Input ──→ Frontend (REPL) ──→ Engine (Agentic Loop)
                   ↑                      │
                   │                      ├──→ LLM Provider (stream)
                   │                      ├──→ Tool Registry (execute)
             EngineEvent              ├──→ Permission Gate
                   │                      └──→ Session Snapshot
                   ↑                      │
                   └──────────────────────┘
```

The **Engine** and **Frontend** communicate exclusively through typed channels (`UserAction` / `EngineEvent`), ensuring a clean separation of concerns.

## License

MIT
