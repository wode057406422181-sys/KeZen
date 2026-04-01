# ⚡ Infini

A fast, modern AI coding CLI built in Rust.

## Overview

Infini is a blazing-fast AI coding assistant built from the ground up in Rust. It provides an interactive terminal interface for AI-assisted development.

## Features

- 🚀 **Blazing fast** — Native Rust performance with zero-cost abstractions
- 🎨 **Beautiful TUI** — Rich terminal UI powered by Ratatui + Crossterm
- 🔧 **Extensible** — Modular architecture for easy tool integration
- 🔒 **Type safe** — Leveraging Rust's type system for reliability
- ⚡ **Async-first** — Built on Tokio for efficient I/O

## Getting Started

### Prerequisites

- Rust 1.94+ (install via [rustup](https://rustup.rs/))

### Build & Run

```bash
# Build
cargo build

# Run
cargo run

# Run with a specific command
cargo run -- chat --prompt "Hello!"

# Show help
cargo run -- --help
```

### Install locally

```bash
cargo install --path .
infini --help
```

## Project Structure

```
src/
├── main.rs          # Entry point, tracing init, CLI dispatch
├── cli.rs           # CLI argument definitions (clap derive)
├── config.rs        # Configuration loading/saving (TOML)
├── error.rs         # Custom error types (thiserror)
└── commands/
    ├── mod.rs       # Command dispatcher
    └── chat.rs      # Interactive chat REPL
```

## License

MIT
