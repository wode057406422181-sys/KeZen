//! Safety checks for file paths and shell commands.
//!
//! Features:
//! - Dangerous file/directory detection
//! - Working-directory constraints
//! - Path traversal prevention
//! - Read-only command recognition

use std::path::{Path, PathBuf};

// ── Dangerous paths ──────────────────────────────────────────────────

/// Files that should never be auto-edited without explicit permission.
const DANGEROUS_FILES: &[&str] = &[
    ".gitconfig",
    ".gitmodules",
    ".bashrc",
    ".bash_profile",
    ".zshrc",
    ".zprofile",
    ".profile",
    ".kezen.md",
];

/// Directories whose contents should always prompt for permission.
const DANGEROUS_DIRECTORIES: &[&str] = &[".git", ".vscode", ".idea"];

/// Returns `true` if the path points to a dangerous file or is inside a
/// dangerous directory (e.g. `.git/hooks/post-commit`).
pub fn is_dangerous_path(path: &str) -> bool {
    let p = Path::new(path);

    // Check filename
    if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
        let lower = name.to_lowercase();
        if DANGEROUS_FILES.iter().any(|f| lower == *f) {
            return true;
        }
    }

    // Check path components for dangerous directories
    for component in p.components() {
        if let std::path::Component::Normal(c) = component
            && let Some(s) = c.to_str()
        {
            let lower = s.to_lowercase();
            if DANGEROUS_DIRECTORIES.iter().any(|d| lower == *d) {
                return true;
            }
        }
    }

    false
}

// ── Path traversal ───────────────────────────────────────────────────

/// Returns `true` if the path contains `..` traversal components.
pub fn contains_path_traversal(path: &str) -> bool {
    Path::new(path)
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
}

// ── Working directory ────────────────────────────────────────────────

/// Returns `true` if `path` is inside `working_dir`.
///
/// Both paths are canonicalized; failure to canonicalize the working dir
/// falls back to a simple `starts_with` check on the raw strings.
pub async fn is_within_working_directory(path: &str, working_dir: &str) -> bool {
    let canonical_wd = tokio::fs::canonicalize(working_dir).await;
    let canonical_path = tokio::fs::canonicalize(path).await;

    match (canonical_path, canonical_wd) {
        (Ok(cp), Ok(cwd)) => cp.starts_with(&cwd),
        _ => {
            // Try canonicalizing the parent directory (which should exist for writes)
            if let Some(parent) = Path::new(path).parent()
                && let Ok(cp) = tokio::fs::canonicalize(parent).await
                && let Ok(cwd) = tokio::fs::canonicalize(working_dir).await
            {
                return cp.starts_with(&cwd);
            }
            false // Fail closed
        }
    }
}

// ── Read-only command detection ──────────────────────────────────────

/// Common commands that are read-only and safe to auto-approve.
const READ_ONLY_COMMANDS: &[&str] = &[
    "ls", "ll", "la", "dir",
    "cat", "head", "tail", "less", "more",
    "grep", "egrep", "fgrep", "rg", "ag",
    "find", "fd", "locate",
    "wc", "sort", "uniq", "diff", "comm",
    "echo", "printf", "date", "cal",
    "pwd", "whoami", "hostname", "uname",
    "env", "printenv",
    "which", "where", "type", "file",
    "du", "df", "free",
    "ps", "top", "uptime",
    "git status", "git log", "git diff", "git show", "git branch",
    "git remote", "git tag", "git stash list",
    "cargo check", "cargo clippy", "cargo test", "cargo bench",
    "rustc --version", "cargo --version",
    "node --version", "npm --version", "npx --version",
    "python --version", "python3 --version", "pip --version",
];

/// Returns `true` if the command is recognized as read-only.
///
/// Matching is prefix-based for multi-word entries (e.g. `git status foo`
/// matches `git status`).
pub fn is_read_only_command(command: &str) -> bool {
    // Reject if it contains shell operators
    if command.contains(';') || command.contains("&&") || command.contains("||") 
        || command.contains('|') || command.contains('`') || command.contains("$(") {
        return false;
    }

    let trimmed = command.trim();
    READ_ONLY_COMMANDS.iter().any(|&ro| {
        // Exact match or prefix match (command starts with ro + space/end)
        trimmed == ro
            || trimmed.starts_with(&format!("{} ", ro))
    })
}

// ── Permission suggestion extraction ─────────────────────────────────

/// Extract a short "command prefix" for use as an always-allow suggestion.
///
/// Examples:
/// - `"git commit -m 'fix typo'"` → `Some("git commit:*")`
/// - `"ls -la"` → `None` (read-only, no suggestion needed)
/// - `"rm -rf /tmp/foo"` → `Some("rm:*")`
pub fn extract_bash_suggestion(command: &str) -> Option<String> {
    let tokens: Vec<&str> = command.split_whitespace().collect();
    if tokens.is_empty() {
        return None;
    }

    let cmd = tokens[0];

    // Multi-word commands (git commit, npm run, cargo build, etc.)
    if tokens.len() >= 2 {
        let subcmd = tokens[1];
        // Only if second token looks like a subcommand (lowercase alpha)
        if subcmd.chars().all(|c| c.is_ascii_lowercase() || c == '-') && !subcmd.starts_with('-') {
            return Some(format!("{} {}:*", cmd, subcmd));
        }
    }

    Some(format!("{}:*", cmd))
}

/// Extract a file-path suggestion for always-allow rules.
///
/// Uses the directory of the file as the pattern.
/// E.g. `/home/user/project/src/main.rs` → `Some("src/**")`
pub fn extract_file_suggestion(file_path: &str, working_dir: &str) -> Option<String> {
    let path = Path::new(file_path);
    let wd = Path::new(working_dir);

    if let Ok(rel) = path.strip_prefix(wd) {
        // Use the first directory component as the suggestion
        let mut components = rel.components();
        if let Some(first) = components.next()
            && let Some(s) = first.as_os_str().to_str()
        {
            // If the file is directly in the working dir root, suggest the file itself
            components.next()?;
            return Some(format!("{}/**", s));
        }
    }

    None
}

// ── Shared file tool capabilities ─────────────────────────────────────

/// Validate file path safety and working directory constraints.
///
/// # Arguments
/// - `file_path`: Path to check (may be relative or absolute)
/// - `work_dir`: Baseline working directory for path boundary checks
///
/// # Security
/// - Rejects directory traversal attempts (`..`)
/// - Asks for confirmation on critical system/project files
/// - Asks for confirmation if the computed path escapes `work_dir`.
pub async fn check_file_permissions(file_path: &str, work_dir: &Path) -> crate::permissions::PermissionResult {
    // Path traversal → deny
    if contains_path_traversal(file_path) {
        return crate::permissions::PermissionResult::Deny {
            message: format!("Path contains traversal (..): {}", file_path),
        };
    }

    // Dangerous files → ask
    if is_dangerous_path(file_path) {
        return crate::permissions::PermissionResult::Ask {
            message: format!("⚠️ Target is a sensitive file: {}", file_path),
        };
    }

    // Working directory check
    let wd_str = work_dir.to_string_lossy();
    if !is_within_working_directory(file_path, &wd_str).await {
        return crate::permissions::PermissionResult::Ask {
            message: format!("⚠️ File is outside the working directory: {}", file_path),
        };
    }

    crate::permissions::PermissionResult::Passthrough
}

pub fn file_permission_matcher(path: String, work_dir: PathBuf) -> Box<dyn Fn(&str) -> bool> {
    Box::new(move |pattern: &str| {
        if let Some(dir) = pattern.strip_suffix("/**") {
            let prefix = if dir.starts_with('/') {
                format!("{}/", dir)
            } else {
                format!("{}/{}/", work_dir.display(), dir)
            };
            path.starts_with(&prefix)
        } else {
            path == pattern
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dangerous_path_detection() {
        assert!(is_dangerous_path("/project/.git/hooks/post-commit"));
        assert!(is_dangerous_path("/home/user/.bashrc"));
        assert!(is_dangerous_path("/project/.vscode/settings.json"));
        assert!(!is_dangerous_path("/project/src/main.rs"));
        assert!(!is_dangerous_path("/project/README.md"));
    }

    #[test]
    fn test_path_traversal() {
        assert!(contains_path_traversal("/project/../etc/passwd"));
        assert!(contains_path_traversal("../../secret"));
        assert!(!contains_path_traversal("/project/src/main.rs"));
        assert!(!contains_path_traversal("src/main.rs"));
    }

    #[test]
    fn test_read_only_commands() {
        assert!(is_read_only_command("ls -la"));
        assert!(is_read_only_command("cat foo.txt"));
        assert!(is_read_only_command("git status"));
        assert!(is_read_only_command("git log --oneline"));
        assert!(is_read_only_command("cargo test"));
        assert!(!is_read_only_command("rm -rf /tmp"));
        assert!(!is_read_only_command("git push"));
        assert!(!is_read_only_command("cargo publish"));
    }

    #[test]
    fn test_bash_suggestion() {
        assert_eq!(extract_bash_suggestion("git commit -m 'fix'"), Some("git commit:*".into()));
        assert_eq!(extract_bash_suggestion("npm run build"), Some("npm run:*".into()));
        assert_eq!(extract_bash_suggestion("rm -rf /tmp"), Some("rm:*".into()));
        assert_eq!(extract_bash_suggestion("ls"), Some("ls:*".into()));
    }

    #[test]
    fn test_file_suggestion() {
        assert_eq!(extract_file_suggestion("/project/src/main.rs", "/project"), Some("src/**".into()));
        assert_eq!(extract_file_suggestion("/project/README.md", "/project"), None);
    }
}
