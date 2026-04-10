use std::path::PathBuf;

use crate::constants::defaults::MAX_MEMORY_CHARACTER_COUNT;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryType {
    User,
    Project,
    Local,
}

#[derive(Debug, Clone)]
pub struct MemoryFile {
    pub path: PathBuf,
    pub memory_type: MemoryType,
    pub content: String,
    pub globs: Option<Vec<String>>,
}

// Extract `paths:` array from YAML frontmatter and return the rest of the text
fn parse_frontmatter(text: &str) -> (Option<Vec<String>>, String) {
    if !text.starts_with("---\n") && !text.starts_with("---\r\n") {
        return (None, text.to_string());
    }

    let start_idx = text.find('\n').unwrap() + 1;

    let end_relative_idx = match text[start_idx..].find("\n---") {
        Some(i) => i,
        None => return (None, text.to_string()),
    };

    let end_idx = start_idx + end_relative_idx;
    let frontmatter = &text[start_idx..end_idx];

    let mut content_start = end_idx + 4;
    if text[content_start..].starts_with("\r\n") {
        content_start += 2;
    } else if text[content_start..].starts_with('\n') {
        content_start += 1;
    }

    let content = &text[content_start..];

    let mut paths = Vec::new();
    let mut in_paths = false;
    for line in frontmatter.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("paths:") {
            in_paths = true;
            continue;
        }
        if in_paths {
            if let Some(stripped) = trimmed.strip_prefix("- ") {
                let p = stripped.trim_matches(|c| c == '"' || c == '\'' || c == ' ');
                if !p.is_empty() {
                    paths.push(p.to_string());
                }
            } else if !trimmed.is_empty() && !trimmed.starts_with('#') {
                // If we encounter another key, break out
                in_paths = false;
            }
        }
    }

    let globs = if paths.is_empty() { None } else { Some(paths) };
    (globs, content.trim_start().to_string())
}

async fn load_memory_file(path: PathBuf, memory_type: MemoryType) -> Option<MemoryFile> {
    if let Ok(mut text) = tokio::fs::read_to_string(&path).await {
        if text.len() > MAX_MEMORY_CHARACTER_COUNT {
            // Truncate to word boundary or just limit chars
            let mut byte_idx = MAX_MEMORY_CHARACTER_COUNT.min(text.len());
            while !text.is_char_boundary(byte_idx) && byte_idx > 0 {
                byte_idx -= 1;
            }
            text.truncate(byte_idx);
            text.push_str("\n\n[Warning: Memory file exceeded 40k characters and was truncated]");
        }

        let (globs, content) = parse_frontmatter(&text);

        Some(MemoryFile {
            path,
            memory_type,
            content,
            globs,
        })
    } else {
        None
    }
}

pub async fn load_memory_files(work_dir: &std::path::Path) -> Vec<MemoryFile> {
    let mut results = Vec::new();

    // 1. User Layer: ~/.kezen/.kezen.md
    if let Some(home) = dirs::home_dir() {
        let user_mem = home.join(".kezen").join(".kezen.md");
        if tokio::fs::try_exists(&user_mem).await.unwrap_or(false)
            && let Some(mf) = load_memory_file(user_mem, MemoryType::User).await
        {
            results.push(mf);
        }
    }

    // 2. Project Layers
    let cwd = work_dir;

    // Find git root via async command
    let git_root = match tokio::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(&cwd)
        .output()
        .await
    {
        Ok(out) if out.status.success() => {
            let root_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
            PathBuf::from(root_str)
        }
        _ => cwd.to_path_buf(),
    };

    let mut traverse_dirs = Vec::new();
    let mut current = cwd;
    loop {
        traverse_dirs.push(current.to_path_buf());
        if current == git_root {
            break;
        }
        match current.parent() {
            Some(p) => current = p,
            None => break,
        }
    }
    // Reverse so we process from root to cwd
    traverse_dirs.reverse();

    for dir in traverse_dirs {
        // Project: .kezen.md
        let prj_md = dir.join(".kezen.md");
        if tokio::fs::try_exists(&prj_md).await.unwrap_or(false)
            && let Some(mf) = load_memory_file(prj_md, MemoryType::Project).await
        {
            results.push(mf);
        }

        // Project Rules: .kezen/rules/*.md
        let rules_dir = dir.join(".kezen").join("rules");
        if tokio::fs::try_exists(&rules_dir).await.unwrap_or(false)
            && let Ok(mut entries) = tokio::fs::read_dir(&rules_dir).await
        {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "md")
                    && let Some(mf) = load_memory_file(path, MemoryType::Project).await
                {
                    results.push(mf);
                }
            }
        }

        // Local: .kezen.local.md
        let local_md = dir.join(".kezen.local.md");
        if tokio::fs::try_exists(&local_md).await.unwrap_or(false)
            && let Some(mf) = load_memory_file(local_md, MemoryType::Local).await
        {
            results.push(mf);
        }
    }

    results
}

pub fn format_memory_prompt(files: &[MemoryFile]) -> Option<String> {
    if files.is_empty() {
        return None;
    }

    // TODO: Implement conditional rule matching — when the engine operates on a file,
    // check if its path matches any MemoryFile.globs and inject those rules too.
    // For now, exclude rules that have specific `paths:` (conditional rules).
    let global_files: Vec<_> = files.iter().filter(|f| f.globs.is_none()).collect();
    if global_files.is_empty() {
        return None;
    }

    let mut out = String::new();
    out.push_str("Codebase and user instructions are shown below. Be sure to adhere to\n");
    out.push_str("these instructions. IMPORTANT: These instructions OVERRIDE any default\n");
    out.push_str("behavior and you MUST follow them exactly as written.\n\n");

    for f in global_files {
        let tp = match f.memory_type {
            MemoryType::User => "user's private global instructions",
            MemoryType::Project => "project instructions, checked into the codebase",
            MemoryType::Local => "user's private project instructions, not checked in",
        };
        out.push_str(&format!("Contents of {} ({}):\n\n", f.path.display(), tp));
        out.push_str(&f.content);
        out.push_str("\n\n");
    }

    Some(out.trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_frontmatter tests ──────────────────────────────────────

    #[test]
    fn test_parse_no_frontmatter() {
        let text = "Just regular content\nNo frontmatter here.";
        let (globs, content) = parse_frontmatter(text);
        assert!(globs.is_none());
        assert_eq!(content, text);
    }

    #[test]
    fn test_parse_frontmatter_with_paths() {
        let text = "---\npaths:\n- \"src/**/*.rs\"\n- tests/\n---\nBody content here.";
        let (globs, content) = parse_frontmatter(text);
        assert_eq!(
            globs,
            Some(vec!["src/**/*.rs".to_string(), "tests/".to_string()])
        );
        assert_eq!(content, "Body content here.");
    }

    #[test]
    fn test_parse_frontmatter_without_paths() {
        let text = "---\ntitle: My Rules\n---\nBody content.";
        let (globs, content) = parse_frontmatter(text);
        assert!(globs.is_none());
        assert_eq!(content, "Body content.");
    }

    #[test]
    fn test_parse_frontmatter_no_end_marker() {
        // Incomplete frontmatter (no closing ---) → treated as no frontmatter
        let text = "---\npaths:\n- foo\n";
        let (globs, content) = parse_frontmatter(text);
        // Without a closing ---, there's no valid frontmatter
        assert!(globs.is_none());
        assert_eq!(content, text);
    }

    // ── format_memory_prompt tests ───────────────────────────────────

    #[test]
    fn test_format_empty_files() {
        assert_eq!(format_memory_prompt(&[]), None);
    }

    #[test]
    fn test_format_global_files_only() {
        let files = vec![MemoryFile {
            path: PathBuf::from("/home/user/.kezen.md"),
            memory_type: MemoryType::User,
            content: "Always use tabs.".into(),
            globs: None,
        }];
        let result = format_memory_prompt(&files).unwrap();
        assert!(result.contains("Always use tabs."));
        assert!(result.contains("user's private global instructions"));
    }

    #[test]
    fn test_format_filters_out_conditional() {
        // File with globs should be excluded
        let files = vec![MemoryFile {
            path: PathBuf::from("/project/.kezen/rules/rust.md"),
            memory_type: MemoryType::Project,
            content: "Use unwrap sparingly.".into(),
            globs: Some(vec!["src/**/*.rs".into()]),
        }];
        assert_eq!(format_memory_prompt(&files), None);
    }

    #[test]
    fn test_format_mixed_global_and_conditional() {
        let files = vec![
            MemoryFile {
                path: PathBuf::from("/home/user/.kezen.md"),
                memory_type: MemoryType::User,
                content: "Global rule.".into(),
                globs: None,
            },
            MemoryFile {
                path: PathBuf::from("/project/.kezen/rules/ts.md"),
                memory_type: MemoryType::Project,
                content: "TypeScript only.".into(),
                globs: Some(vec!["*.ts".into()]),
            },
        ];
        let result = format_memory_prompt(&files).unwrap();
        assert!(result.contains("Global rule."));
        assert!(!result.contains("TypeScript only."));
    }

    #[test]
    fn test_format_project_type_label() {
        let files = vec![MemoryFile {
            path: PathBuf::from("/project/.kezen.md"),
            memory_type: MemoryType::Project,
            content: "Project rules.".into(),
            globs: None,
        }];
        let result = format_memory_prompt(&files).unwrap();
        assert!(result.contains("project instructions, checked into the codebase"));
    }

    #[test]
    fn test_format_local_type_label() {
        let files = vec![MemoryFile {
            path: PathBuf::from("/project/.kezen.local.md"),
            memory_type: MemoryType::Local,
            content: "Local rules.".into(),
            globs: None,
        }];
        let result = format_memory_prompt(&files).unwrap();
        assert!(result.contains("user's private project instructions, not checked in"));
    }

    // ── load_memory_file tests ───────────────────────────────────────

    #[tokio::test]
    async fn test_load_memory_file_basic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".kezen.md");
        std::fs::write(&path, "Use Rust best practices.").unwrap();

        let mf = load_memory_file(path.clone(), MemoryType::User)
            .await
            .unwrap();
        assert_eq!(mf.memory_type, MemoryType::User);
        assert_eq!(mf.content, "Use Rust best practices.");
        assert!(mf.globs.is_none());
    }

    #[tokio::test]
    async fn test_load_memory_file_with_frontmatter() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.md");
        std::fs::write(&path, "---\npaths:\n- \"*.rs\"\n---\nRust rules.").unwrap();

        let mf = load_memory_file(path, MemoryType::Project).await.unwrap();
        assert_eq!(mf.globs, Some(vec!["*.rs".to_string()]));
        assert_eq!(mf.content, "Rust rules.");
    }

    #[tokio::test]
    async fn test_load_memory_file_nonexistent() {
        let result =
            load_memory_file(PathBuf::from("/nonexistent/path.md"), MemoryType::User).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_load_memory_file_truncation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.md");
        let content = "x".repeat(50_000);
        std::fs::write(&path, &content).unwrap();

        let mf = load_memory_file(path, MemoryType::User).await.unwrap();
        assert!(mf.content.len() < 50_000);
        assert!(
            mf.content
                .contains("[Warning: Memory file exceeded 40k characters and was truncated]")
        );
    }
}
