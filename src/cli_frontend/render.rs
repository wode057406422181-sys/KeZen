use colored::Colorize;
use termimad::MadSkin;

use crate::api::types::Usage;
use crate::config::AppConfig;

/// Print the welcome banner with provider and model info.
pub fn print_welcome(config: &AppConfig) {
    println!(
        "\n  {} {}",
        "⚡".bold(),
        "Infini".bright_cyan().bold()
    );
    println!(
        "  Provider: {} | Model: {}",
        config.provider,
        config.model.as_deref().unwrap_or("(not set)")
    );
    println!();
}

/// Render a complete markdown string to the terminal using termimad.
#[allow(dead_code)]
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
