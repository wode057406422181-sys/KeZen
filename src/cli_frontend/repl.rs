use std::io::Write;

use colored::Colorize;
use rustyline::DefaultEditor;
use tokio::sync::mpsc;

use crate::config::AppConfig;
use crate::engine::events::{EngineEvent, UserAction};

use super::render::{print_ai_prefix, print_cost, print_error, print_thinking, print_welcome, print_tool_use, print_tool_result};

/// Run the interactive REPL loop.
///
/// If `initial_prompt` is Some, sends that single message, prints the response, and exits.
/// Otherwise enters the interactive read-eval-print loop.
pub async fn run_repl(
    config: AppConfig,
    action_tx: mpsc::Sender<UserAction>,
    mut event_rx: mpsc::Receiver<EngineEvent>,
    initial_prompt: Option<String>,
) -> anyhow::Result<()> {
    print_welcome(&config);

    // Single-prompt (non-interactive) mode
    if let Some(prompt) = initial_prompt {
        println!("  {} {}", "→".bright_green(), prompt);
        action_tx
            .send(UserAction::SendMessage { content: prompt })
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send message: {}", e))?;
        handle_engine_events(&action_tx, &mut event_rx).await;
        return Ok(());
    }

    // Interactive REPL mode
    let mut rl = DefaultEditor::new()?;

    loop {
        let readline = rl.readline(&format!("  {} ", "›".bright_green().bold()));
        match readline {
            Ok(line) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                let _ = rl.add_history_entry(trimmed);

                // Handle slash commands
                match trimmed {
                    "/quit" | "/exit" => {
                        println!("\n  👋 {}\n", "Goodbye!".dimmed());
                        break;
                    }
                    "/help" => {
                        print_help();
                        continue;
                    }
                    "/clear" => {
                        println!(
                            "  {} {}",
                            "ℹ".blue(),
                            "Session clear is not yet implemented.".dimmed()
                        );
                        continue;
                    }
                    "/model" => {
                        println!(
                            "  {} Model: {}",
                            "ℹ".blue(),
                            config.model.as_deref().unwrap_or("(not set)")
                        );
                        continue;
                    }
                    _ if trimmed.starts_with('/') => {
                        println!(
                            "  {} Unknown command: {}. Type /help for available commands.",
                            "?".yellow(),
                            trimmed
                        );
                        continue;
                    }
                    _ => {}
                }

                // Send trimmed message to engine (stripping surrounding whitespace).
                if action_tx
                    .send(UserAction::SendMessage {
                        content: trimmed.to_string(),
                    })
                    .await
                    .is_err()
                {
                    eprintln!("  {} Engine disconnected.", "✖".red());
                    break;
                }

                // Handle the streaming response
                handle_engine_events(&action_tx, &mut event_rx).await;
            }
            Err(rustyline::error::ReadlineError::Interrupted) => {
                // Ctrl-C at the prompt: just print and continue
                println!("^C");
            }
            Err(rustyline::error::ReadlineError::Eof) => {
                // Ctrl-D: exit
                println!("\n  👋 {}\n", "Goodbye!".dimmed());
                break;
            }
            Err(err) => {
                print_error(&format!("Input error: {}", err));
                break;
            }
        }
    }

    Ok(())
}

/// Consume engine events until Done or channel closed.
///
/// During streaming, Ctrl+C sends a Cancel action to the engine.
async fn handle_engine_events(
    action_tx: &mpsc::Sender<UserAction>,
    event_rx: &mut mpsc::Receiver<EngineEvent>,
) {
    let mut in_thinking = false;
    print_ai_prefix();

    loop {
        tokio::select! {
            biased;

            // Ctrl+C sends Cancel (interrupt stream, don't exit)
            _ = tokio::signal::ctrl_c() => {
                println!("\n  {} {}", "✖".red(), "Cancelled".dimmed());
                let _ = action_tx.send(UserAction::Cancel).await;
                // Continue waiting for the Done event from engine
            }

            evt_opt = event_rx.recv() => {
                match evt_opt {
                    Some(EngineEvent::ThinkingDelta { text }) => {
                        if !in_thinking {
                            in_thinking = true;
                        }
                        print_thinking(&text);
                        let _ = std::io::stdout().flush();
                    }
                    Some(EngineEvent::TextDelta { text }) => {
                        if in_thinking {
                            in_thinking = false;
                            // Newline to separate thinking from response
                            println!();
                            print_ai_prefix();
                        }
                        print!("{}", text);
                        let _ = std::io::stdout().flush();
                    }
                    Some(EngineEvent::CostUpdate(usage)) => {
                        print_cost(&usage);
                    }
                    Some(EngineEvent::Error { message }) => {
                        print_error(&message);
                        break;
                    }
                    Some(EngineEvent::ToolUseStart { id: _, name, input }) => {
                        if in_thinking {
                            in_thinking = false;
                            println!();
                        }
                        print_tool_use(&name, &input);
                    }
                    Some(EngineEvent::ToolResult { id: _, output, is_error }) => {
                        print_tool_result(&output, is_error);
                    }
                    Some(EngineEvent::Done) => {
                        println!(); // Final newline
                        break;
                    }
                    None => {
                        // Channel closed, engine died
                        print_error("Engine disconnected unexpectedly.");
                        break;
                    }
                }
            }
        }
    }
}

/// Print available slash commands.
fn print_help() {
    println!();
    println!("  {}", "Available Commands:".bold());
    println!("  {}  — Show this help message", "/help".cyan());
    println!("  {}  — Exit Infini", "/quit".cyan());
    println!("  {} — Clear conversation history", "/clear".cyan());
    println!("  {} — Show current model", "/model".cyan());
    println!();
    println!("  {}", "Keyboard Shortcuts:".bold());
    println!("  {}  — Cancel current response", "Ctrl+C".cyan());
    println!("  {}  — Exit Infini", "Ctrl+D".cyan());
    println!();
}
