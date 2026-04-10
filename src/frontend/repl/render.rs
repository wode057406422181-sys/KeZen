use colored::Colorize;
use termimad::MadSkin;

use crate::api::types::{ContentBlock, Message, Role, Usage};
use crate::config::AppConfig;
use crate::constants::limits::*;

/// Print the welcome banner with provider and model info.
pub fn print_welcome(config: &AppConfig) {
    println!(
        "\n  {} {}",
        "⚡".bold(),
        "KeZen — AI Coding Assistant".bright_cyan().bold()
    );
    println!(
        "  Provider: {} | Model: {}",
        config.provider,
        config.model.as_deref().unwrap_or("(not set)")
    );
    println!();
}

/// Render a complete markdown string to the terminal using termimad.
pub fn render_markdown(text: &str) {
    let skin = MadSkin::default();
    skin.print_text(text);
}

/// Print the AI response prefix marker.
pub fn print_ai_prefix() {
    print!("\n  {} ", "⟡".cyan());
}

/// Print thinking text in dim/italic style.
pub fn print_thinking(text: &str) {
    print!("{}", text.dimmed());
}

/// Print an error message in red.
pub fn print_error(msg: &str) {
    eprintln!("\n  {} {}", "✖".red().bold(), msg.red());
}

/// Print token usage summary for a turn.
pub fn print_cost(usage: &Usage) {
    let total = usage.input_tokens + usage.output_tokens;
    println!(
        "\n  {}",
        format!(
            "tokens: {} in / {} out | total: {}",
            usage.input_tokens, usage.output_tokens, total
        )
        .dimmed()
    );
}

/// Print tool call indicator
pub fn print_tool_use(name: &str, input: &serde_json::Value) {
    let input_str = serde_json::to_string(input).unwrap_or_default();
    println!("\n  {} {} {}", "🔧".blue(), name.bold(), input_str.dimmed());
}

/// Print tool result preview
pub fn print_tool_result(output: &str, is_error: bool) {
    let limit = UI_MAX_TOOL_RESULT_HISTORY_CHARS;
    let single_line = output.replace('\n', " ");
    let preview = if single_line.chars().count() > limit {
        let byte_end = single_line
            .char_indices()
            .nth(limit)
            .map(|(i, _)| i)
            .unwrap_or(single_line.len());
        format!("{}...", &single_line[..byte_end])
    } else {
        single_line
    };

    if is_error {
        println!("  {} {}", "✖".red(), preview.red());
    } else {
        println!("  {} {}", "✓".green(), preview.dimmed());
    }
}

/// Print permission request
pub fn print_permission_request(tool: &str, desc: &str) {
    println!("\n  {} {} wants to execute:", "⚠".yellow().bold(), tool.bold());
    println!("     {}", desc);
}

/// Render restored session messages to the terminal.
///
/// Each message is printed with a role prefix and its content blocks
/// formatted according to type (text, thinking, tool_use, tool_result).
pub fn render_restored_messages(messages: &[Message]) {
    println!("  {} {}", "📜".bold(), "Restored session history:".dimmed());
    println!("  {}", "─".repeat(60).dimmed());
    for msg in messages {
        let prefix = match msg.role {
            Role::User => format!("  {} ", "→".bright_green()),
            Role::Assistant => format!("  {} ", "⟡".cyan()),
            Role::System => format!("  {} ", "⚙".dimmed()),
        };

        for block in &msg.content {
            match block {
                ContentBlock::Text { text } => {
                    // Truncate long messages in history view
                    let display = if text.chars().count() > UI_MAX_TEXT_CHARS {
                        let byte_end = text
                            .char_indices()
                            .nth(UI_MAX_TEXT_CHARS)
                            .map(|(i, _)| i)
                            .unwrap_or(text.len());
                        format!("{}...", &text[..byte_end])
                    } else {
                        text.clone()
                    };
                    // First line with prefix, rest indented
                    let lines: Vec<&str> = display.lines().collect();
                    if let Some(first) = lines.first() {
                        println!("{}{}", prefix, first);
                        for line in &lines[1..] {
                            println!("    {}", line);
                        }
                    }
                }
                ContentBlock::Thinking { thinking } => {
                    let preview = if thinking.chars().count() > UI_MAX_THINKING_CHARS {
                        let byte_end = thinking
                            .char_indices()
                            .nth(UI_MAX_THINKING_CHARS)
                            .map(|(i, _)| i)
                            .unwrap_or(thinking.len());
                        format!("{}...", &thinking[..byte_end])
                    } else {
                        thinking.clone()
                    };
                    println!("{}💭 {}", prefix, preview.dimmed());
                }
                ContentBlock::ToolUse { name, input, .. } => {
                    let input_preview = serde_json::to_string(input).unwrap_or_default();
                    let preview = if input_preview.chars().count() > UI_MAX_TOOL_INPUT_CHARS {
                        let byte_end = input_preview
                            .char_indices()
                            .nth(UI_MAX_TOOL_INPUT_CHARS)
                            .map(|(i, _)| i)
                            .unwrap_or(input_preview.len());
                        format!("{}...", &input_preview[..byte_end])
                    } else {
                        input_preview
                    };
                    println!("  {} {} {}", "🔧".blue(), name.bold(), preview.dimmed());
                }
                ContentBlock::ToolResult { content, is_error, .. } => {
                    let preview = if content.chars().count() > UI_MAX_TOOL_RESULT_HISTORY_CHARS {
                        let byte_end = content
                            .char_indices()
                            .nth(UI_MAX_TOOL_RESULT_HISTORY_CHARS)
                            .map(|(i, _)| i)
                            .unwrap_or(content.len());
                        format!("{}...", &content[..byte_end])
                    } else {
                        content.clone()
                    };
                    let single_line = preview.replace('\n', " ");
                    if *is_error {
                        println!("  {} {}", "✖".red(), single_line.red());
                    } else {
                        println!("  {} {}", "✓".green(), single_line.dimmed());
                    }
                }
            }
        }
    }
    println!("  {}", "─".repeat(60).dimmed());
    println!();
}
