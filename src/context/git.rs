use std::path::Path;
use std::sync::Arc;
use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct GitContext {
    pub branch: String,
    pub default_branch: String,
    pub status: String,
    pub recent_commits: String,
    #[allow(dead_code)] // TODO: Inject git user name into prompt for personalized responses
    pub user_name: Option<String>,
}

pub async fn collect_git_context(work_dir: &Path) -> Option<GitContext> {
    let cwd: Arc<Path> = Arc::from(work_dir);

    // Quick check: are we inside a git work tree?
    let ok = git(&cwd, &["rev-parse", "--is-inside-work-tree"])
        .await
        .is_some_and(|s| s == "true");
    if !ok {
        return None;
    }

    // Run all queries concurrently.
    let (branch, default_branch, status, recent_commits, user_name) = tokio::join!(
        git_branch(&cwd),
        git_default_branch(&cwd),
        git_status(&cwd),
        git_recent_commits(&cwd),
        git_user_name(&cwd),
    );

    Some(GitContext { branch, default_branch, status, recent_commits, user_name })
}

// ── Individual queries ──────────────────────────────────────────────

async fn git_branch(cwd: &Path) -> String {
    git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"])
        .await
        .unwrap_or_default()
}

async fn git_default_branch(cwd: &Path) -> String {
    let remote = git(cwd, &["remote"]).await.unwrap_or_default();
    let first_remote = remote.lines().next().unwrap_or("origin");

    let full = git(
        cwd,
        &["symbolic-ref", "--short", &format!("refs/remotes/{first_remote}/HEAD")],
    )
    .await
    .unwrap_or_default();

    full.rsplit('/').next().unwrap_or("main").to_string()
}

async fn git_status(cwd: &Path) -> String {
    let mut st = git(cwd, &["status", "-s", "-b"]).await.unwrap_or_default();
    if st.len() > 1000 {
        st.truncate(1000);
        st.push_str("\n... (truncated)");
    }
    st
}

async fn git_recent_commits(cwd: &Path) -> String {
    git(cwd, &["log", "-n", "5", "--oneline"])
        .await
        .unwrap_or_default()
}

async fn git_user_name(cwd: &Path) -> Option<String> {
    git(cwd, &["config", "user.name"])
        .await
        .filter(|s| !s.is_empty())
}

// ── Low-level helper ────────────────────────────────────────────────

async fn git(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .await
        .ok()?;

    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
