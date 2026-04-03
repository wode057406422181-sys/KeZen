use std::fs;
use std::path::PathBuf;

const MAX_MEMORY_CHARACTER_COUNT: usize = 40_000;

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

    let end_idx = text.find("\n---").map(|i| i + 4).unwrap_or(0);
    if end_idx == 0 {
        return (None, text.to_string());
    }

    let frontmatter = &text[..end_idx - 4];
    let content = &text[end_idx..];

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

fn load_memory_file(path: PathBuf, memory_type: MemoryType) -> Option<MemoryFile> {
    if let Ok(mut text) = fs::read_to_string(&path) {
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

pub fn load_memory_files() -> Vec<MemoryFile> {
    let mut results = Vec::new();

    // 1. User Layer: ~/.infini.md
    if let Some(home) = dirs::home_dir() {
        let user_mem = home.join(".infini.md");
        if user_mem.exists()
            && let Some(mf) = load_memory_file(user_mem, MemoryType::User) {
                results.push(mf);
            }
    }

    // 2. Project Layers
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    
    // Find git root via blocking command
    let git_root = match std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(&cwd)
        .output()
    {
        Ok(out) if out.status.success() => {
            let root_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
            PathBuf::from(root_str)
        }
        _ => cwd.clone(),
    };

    let mut traverse_dirs = Vec::new();
    let mut current = cwd.as_path();
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
        // Project: .infini.md
        let prj_md = dir.join(".infini.md");
        if prj_md.exists()
            && let Some(mf) = load_memory_file(prj_md, MemoryType::Project) {
                results.push(mf);
            }

        // Project Rules: .infini/rules/*.md
        let rules_dir = dir.join(".infini").join("rules");
        if rules_dir.exists() && rules_dir.is_dir()
            && let Ok(entries) = fs::read_dir(&rules_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() && path.extension().is_some_and(|e| e == "md")
                        && let Some(mf) = load_memory_file(path, MemoryType::Project) {
                            results.push(mf);
                        }
                }
            }

        // Local: .infini.local.md
        let local_md = dir.join(".infini.local.md");
        if local_md.exists()
            && let Some(mf) = load_memory_file(local_md, MemoryType::Local) {
                results.push(mf);
            }
    }

    results
}

pub fn format_memory_prompt(files: &[MemoryFile]) -> Option<String> {
    if files.is_empty() {
        return None;
    }

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
