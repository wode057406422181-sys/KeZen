use anyhow::Result;
use colored::Colorize;

pub async fn run(initial_prompt: Option<String>) -> Result<()> {
    println!(
        "\n  {} {}\n",
        "⚡".bold(),
        "Infini — AI Coding Assistant".bright_cyan().bold()
    );
    println!(
        "  {}",
        "Type your message and press Enter. Use Ctrl+C to exit."
            .dimmed()
    );
    println!();

    if let Some(prompt) = initial_prompt {
        println!("  {} {}", "→".bright_green(), prompt);
        println!(
            "  {} {}",
            "◆".bright_magenta(),
            "(AI response will appear here)".dimmed()
        );
    } else {
        // Interactive REPL loop
        let stdin = std::io::stdin();
        let mut input = String::new();

        loop {
            print!("  {} ", "›".bright_green().bold());
            // Flush stdout to ensure prompt appears before reading
            use std::io::Write;
            std::io::stdout().flush()?;

            input.clear();
            let bytes_read = stdin.read_line(&mut input)?;

            if bytes_read == 0 {
                // EOF
                break;
            }

            let trimmed = input.trim();
            if trimmed.is_empty() {
                continue;
            }

            if trimmed == "/quit" || trimmed == "/exit" {
                println!(
                    "\n  {} {}\n",
                    "👋",
                    "Goodbye!".dimmed()
                );
                break;
            }

            // TODO: send to AI backend
            println!(
                "  {} {}\n",
                "◆".bright_magenta(),
                "(AI integration pending — message received)".dimmed()
            );
        }
    }

    Ok(())
}
