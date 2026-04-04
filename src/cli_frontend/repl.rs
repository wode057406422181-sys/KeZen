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

    let mut session_in_tokens = 0u64;
    let mut session_out_tokens = 0u64;
    let pricing = crate::cost::get_model_pricing(config.model.as_deref().unwrap_or(""));

    // Single-prompt (non-interactive) mode
    if let Some(prompt) = initial_prompt {
        println!("  {} {}", "→".bright_green(), prompt);
        action_tx
            .send(UserAction::SendMessage { content: prompt })
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send message: {}", e))?;
        handle_engine_events(&action_tx, &mut event_rx, &mut session_in_tokens, &mut session_out_tokens).await;
        
        let cost = crate::cost::calculate_cost(session_in_tokens, session_out_tokens, &pricing);
        println!("\n  {} Session Usage: {} in | {} out | cost: ${:.4}", "ℹ".blue(), session_in_tokens, session_out_tokens, cost);
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

                // Handle REPL-only commands (process control).
                // All other slash commands are delegated to the Engine
                // via handle_slash_command(), which returns results as
                // SlashCommandResult events.
                if trimmed == "/quit" || trimmed == "/exit" {
                    let cost = crate::cost::calculate_cost(session_in_tokens, session_out_tokens, &pricing);
                    println!("\n  {} Session Usage: {} in | {} out | cost: ${:.4}", "ℹ".blue(), session_in_tokens, session_out_tokens, cost);
                    println!("\n  👋 {}\n", "Goodbye!".dimmed());
                    break;
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
                handle_engine_events(&action_tx, &mut event_rx, &mut session_in_tokens, &mut session_out_tokens).await;
            }
            Err(rustyline::error::ReadlineError::Interrupted) => {
                // Ctrl-C at the prompt: just print and continue
                println!("^C");
            }
            Err(rustyline::error::ReadlineError::Eof) => {
                // Ctrl-D: exit
                let cost = crate::cost::calculate_cost(session_in_tokens, session_out_tokens, &pricing);
                println!("\n  {} Session Usage: {} in | {} out | cost: ${:.4}", "ℹ".blue(), session_in_tokens, session_out_tokens, cost);
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
    session_in: &mut u64,
    session_out: &mut u64,
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
                        *session_in += usage.input_tokens;
                        *session_out += usage.output_tokens;
                        print_cost(&usage);
                    }
                    Some(EngineEvent::SessionSnapshotUpdate { snapshot }) => {
                        let _ = crate::session::save_snapshot(&snapshot).await;
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
                    Some(EngineEvent::PermissionRequest { id, tool, description, risk_level, suggestion }) => {
                        if in_thinking {
                            in_thinking = false;
                            println!();
                        }
                        
                        // Display risk level indicator
                        let risk_indicator = match risk_level {
                            crate::permissions::RiskLevel::Low => "○".dimmed(),
                            crate::permissions::RiskLevel::Medium => "●".yellow(),
                            crate::permissions::RiskLevel::High => "●".red(),
                        };
                        println!("  {} {} {}", risk_indicator, "Permission required".bold(), format!("[{}]", tool).dimmed());
                        
                        super::render::print_permission_request(&tool, &description);
                        
                        let (tx, rx) = tokio::sync::oneshot::channel();
                        let tool_name = tool.clone();
                        let suggestion_display = suggestion.clone();
                        tokio::task::spawn_blocking(move || {
                            loop {
                                let always_label = if let Some(ref s) = suggestion_display {
                                    format!("[a] Always allow \"{}:{}\"", tool_name, s)
                                } else {
                                    format!("[a] Always allow {}", tool_name)
                                };
                                print!("  {} [y] Allow [n] Deny {} > ", "›".bright_green().bold(), always_label.bold());
                                let _ = std::io::stdout().flush();
                                let mut input = String::new();
                                if std::io::stdin().read_line(&mut input).is_ok() {
                                    let choice = input.trim().to_lowercase();
                                    match choice.as_str() {
                                        "y" | "yes" => { let _ = tx.send((true, false)); break; }
                                        "n" | "no" => { let _ = tx.send((false, false)); break; }
                                        "a" | "all" | "always" => { let _ = tx.send((true, true)); break; }
                                        _ => { println!("  Please answer 'y', 'n', or 'a'."); }
                                    }
                                } else {
                                    let _ = tx.send((false, false)); break;
                                }
                            }
                        });
                        
                        let (allowed, always_allow) = rx.await.unwrap_or((false, false));
                        let _ = action_tx.send(UserAction::PermissionResponse {
                            id,
                            allowed,
                            always_allow,
                        }).await;
                    }
                    Some(EngineEvent::Done) => {
                        println!(); // Final newline
                        break;
                    }
                    Some(EngineEvent::SlashCommandResult { command, output }) => {
                        println!("  {} {}\n{}", "ℹ".blue(), command.dimmed(), output);
                        break;
                    }
                    Some(EngineEvent::CompactProgress { message }) => {
                        println!("  {} {}", "ℹ".blue(), message.dimmed());
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

