use std::io::Write;

use colored::Colorize;
use rustyline::DefaultEditor;
use tokio::sync::{broadcast, mpsc};

use crate::config::AppConfig;
use crate::engine::events::{EngineEvent, UserAction};

use super::render::{print_ai_prefix, print_cost, print_error, print_thinking, print_welcome, print_tool_use, print_tool_result, render_markdown, render_restored_messages};

/// Run the interactive REPL loop.
///
/// If `initial_prompt` is Some, sends that single message, prints the response, and exits.
/// Otherwise enters the interactive read-eval-print loop.
pub async fn run_repl(
    config: AppConfig,
    action_tx: mpsc::Sender<UserAction>,
    mut event_rx: broadcast::Receiver<EngineEvent>,
    initial_prompt: Option<String>,
) -> anyhow::Result<()> {
    print_welcome(&config);

    let mut session_in_tokens = 0u64;
    let mut session_out_tokens = 0u64;
    let mut session_cache_creation_tokens = 0u64;
    let mut session_cache_read_tokens = 0u64;
    let pricing = crate::cost::get_model_pricing(config.model.as_deref().unwrap_or(""));

    // Single-prompt (non-interactive) mode
    if let Some(prompt) = initial_prompt {
        println!("  {} {}", "→".bright_green(), prompt);
        action_tx
            .send(UserAction::SendMessage { content: prompt })
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send message: {}", e))?;
        handle_engine_events(&action_tx, &mut event_rx, &mut session_in_tokens, &mut session_out_tokens, &mut session_cache_creation_tokens, &mut session_cache_read_tokens).await;
        
        let cost = crate::cost::calculate_cost(session_in_tokens, session_out_tokens, session_cache_creation_tokens, session_cache_read_tokens, &pricing);
        println!("\n  {} Session Usage: {} in | {} out | {} cache creation | {} cache read | cost: ${:.4}", "ℹ".blue(), session_in_tokens, session_out_tokens, session_cache_creation_tokens, session_cache_read_tokens, cost);
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
                    let cost = crate::cost::calculate_cost(session_in_tokens, session_out_tokens, session_cache_creation_tokens, session_cache_read_tokens, &pricing);
                    println!("\n  {} Session Usage: {} in | {} out | {} cache creation | {} cache read | cost: ${:.4}", "ℹ".blue(), session_in_tokens, session_out_tokens, session_cache_creation_tokens, session_cache_read_tokens, cost);
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
                handle_engine_events(&action_tx, &mut event_rx, &mut session_in_tokens, &mut session_out_tokens, &mut session_cache_creation_tokens, &mut session_cache_read_tokens).await;
            }
            Err(rustyline::error::ReadlineError::Interrupted) => {
                // Ctrl-C at the prompt: just print and continue
                println!("^C");
            }
            Err(rustyline::error::ReadlineError::Eof) => {
                // Ctrl-D: exit
                let cost = crate::cost::calculate_cost(session_in_tokens, session_out_tokens, session_cache_creation_tokens, session_cache_read_tokens, &pricing);
                println!("\n  {} Session Usage: {} in | {} out | {} cache creation | {} cache read | cost: ${:.4}", "ℹ".blue(), session_in_tokens, session_out_tokens, session_cache_creation_tokens, session_cache_read_tokens, cost);
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
    event_rx: &mut broadcast::Receiver<EngineEvent>,
    session_in: &mut u64,
    session_out: &mut u64,
    session_cache_creation: &mut u64,
    session_cache_read: &mut u64,
) {
    let mut in_thinking = false;
    // Accumulate the last round's assistant text for markdown rendering.
    // We track it per-round: each time a new ToolUse starts (meaning a new
    // agentic loop iteration), we clear the buffer. Only the final round's
    // text (the one that ends with Done, not tool calls) gets rendered.
    let mut last_round_text = String::new();
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

            result = event_rx.recv() => {
                match result {
                    Ok(EngineEvent::ThinkingDelta { text }) => {
                        if !in_thinking {
                            in_thinking = true;
                        }
                        print_thinking(&text);
                        let _ = std::io::stdout().flush();
                    }
                    Ok(EngineEvent::TextDelta { text }) => {
                        if in_thinking {
                            in_thinking = false;
                            // Newline to separate thinking from response
                            println!();
                            print_ai_prefix();
                        }
                        last_round_text.push_str(&text);
                        print!("{}", text);
                        let _ = std::io::stdout().flush();
                    }
                    Ok(EngineEvent::CostUpdate(usage)) => {
                        *session_in = usage.input_tokens;
                        *session_out = usage.output_tokens;
                        *session_cache_creation = usage.cache_creation_input_tokens;
                        *session_cache_read = usage.cache_read_input_tokens;
                        print_cost(&usage);
                    }

                    Ok(EngineEvent::Error { message }) => {
                        print_error(&message);
                        break;
                    }
                    Ok(EngineEvent::ToolUseStart { id: _, name, input }) => {
                        if in_thinking {
                            in_thinking = false;
                            println!();
                        }
                        // New agentic loop iteration starting — clear last round text.
                        // The text from tool-call iterations is not the final answer.
                        last_round_text.clear();
                        print_tool_use(&name, &input);
                    }
                    Ok(EngineEvent::ToolResult { id: _, output, is_error }) => {
                        print_tool_result(&output, is_error);
                    }
                    Ok(EngineEvent::PermissionRequest { id, tool, description, risk_level, suggestion }) => {
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
                    Ok(EngineEvent::Done) => {
                        // Markdown rendering: if the last round produced text,
                        // render it below a separator line for rich formatting.
                        if !last_round_text.trim().is_empty() {
                            println!();
                            println!("  {}", "─".repeat(60).dimmed());
                            render_markdown(last_round_text.trim());
                        } else {
                            println!(); // Final newline
                        }
                        break;
                    }
                    Ok(EngineEvent::SlashCommandResult { command, output }) => {
                        println!("  {} {}\n{}", "ℹ".blue(), command.dimmed(), output);
                        break;
                    }
                    Ok(EngineEvent::CompactProgress { message }) => {
                        println!("  {} {}", "🗜".magenta(), message.dimmed());
                    }
                    Ok(EngineEvent::Warning(message)) => {
                        println!("  {} {}", "⚠".yellow(), message.yellow());
                    }
                    Ok(EngineEvent::SkillLoaded { name }) => {
                        if in_thinking {
                            in_thinking = false;
                            println!();
                        }
                        println!("  {} {} {}", "⚡".yellow(), "Skill invoked".bold(), format!("[{}]", name).dimmed());
                        let _ = std::io::stdout().flush();
                    }
                    Ok(EngineEvent::SessionRestored { messages }) => {
                        render_restored_messages(&messages);
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("REPL event receiver lagged, skipped {} events", n);
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        // Channel closed, engine died
                        print_error("Engine disconnected unexpectedly.");
                        break;
                    }
                }
            }
        }
    }
}
