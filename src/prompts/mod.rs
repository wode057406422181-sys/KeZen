use std::env;
use std::process::Command;

pub fn build_system_prompt() -> String {
    let os_type = env::consts::OS;
    let os_version = env::consts::FAMILY;
    let shell = env::var("SHELL").unwrap_or_else(|_| "unknown".to_string());
    let cwd = env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    let current_date = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

    let mut prompt = format!(
        "You are Infini, an interactive AI coding assistant running in the user's terminal.

## Environment
- OS: {} {}
- Shell: {}
- Working Directory: {}
- Date: {}",
        os_type, os_version, shell, cwd, current_date
    );

    // Git Status
    if let Ok(output) = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        && output.status.success()
    {
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        prompt.push_str("\n\n## Git Context\n");
        prompt.push_str(&format!("- Branch: {}\n", branch));

        if let Ok(status) = Command::new("git").args(["status", "--short"]).output() {
            let status_str = String::from_utf8_lossy(&status.stdout).trim().to_string();
            if !status_str.is_empty() {
                prompt.push_str("- Status:\n```\n");
                prompt.push_str(&status_str);
                prompt.push_str("\n```\n");
            } else {
                prompt.push_str("- Status: Clean\n");
            }
        }
    }

    // Try reading .infini.md
    if let Ok(content) = std::fs::read_to_string(".infini.md") {
        prompt.push_str("\n\n## Project Memory\n");
        prompt.push_str(&content);
    }

    prompt
}
