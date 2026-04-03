use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct GitContext {
    pub branch: String,
    pub default_branch: String,
    pub status: String,
    pub recent_commits: String,
    #[allow(dead_code)]
    pub user_name: Option<String>,
}

#[allow(dead_code)]
pub async fn collect_git_context() -> Option<GitContext> {
    tokio::task::spawn_blocking(collect_git_context_sync)
        .await
        .unwrap_or(None)
}

pub fn collect_git_context_sync() -> Option<GitContext> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    // Check if git is available and we are in a repo
    let check = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(&cwd)
        .output();
    if check.is_err() || !check.unwrap().status.success() {
        return None;
    }

    let run_cmd = |args: &[&str]| -> String {
        Command::new("git")
            .args(args)
            .current_dir(&cwd)
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
            .unwrap_or_default()
    };

    let (branch, default_branch, status, recent_commits, user_name) = std::thread::scope(|s| {
        let h_branch = s.spawn(|| run_cmd(&["rev-parse", "--abbrev-ref", "HEAD"]));
        let h_default = s.spawn(|| {
            let remote = run_cmd(&["remote"]);
            let first_remote = remote.lines().next().unwrap_or("origin");
            let cmd = run_cmd(&["symbolic-ref", "--short", &format!("refs/remotes/{}/HEAD", first_remote)]);
            let short = cmd.split('/').next_back().unwrap_or("main").to_string();
            if short.is_empty() { "main".to_string() } else { short }
        });
        let h_status = s.spawn(|| {
            let mut st = run_cmd(&["status", "-s", "-b"]);
            if st.len() > 1000 {
                st.truncate(1000);
                st.push_str("\n... (truncated)");
            }
            st
        });
        let h_commits = s.spawn(|| run_cmd(&["log", "-n", "5", "--oneline"]));
        let h_user = s.spawn(|| run_cmd(&["config", "user.name"]));

        (
            h_branch.join().unwrap_or_default(),
            h_default.join().unwrap_or_else(|_| "main".to_string()),
            h_status.join().unwrap_or_default(),
            h_commits.join().unwrap_or_default(),
            h_user.join().ok().filter(|s| !s.is_empty()),
        )
    });

    Some(GitContext {
        branch,
        default_branch,
        status,
        recent_commits,
        user_name,
    })
}
