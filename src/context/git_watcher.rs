use std::sync::Arc;
use tokio::sync::RwLock;

use crate::constants::engine::GIT_WATCHER_INTERVAL_SECS;
use crate::context::git::{GitContext, collect_git_context};

pub struct GitWatcher {
    pub cache: Arc<RwLock<Option<GitContext>>>,
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl GitWatcher {
    /// Start the background watcher and return the watcher instance
    pub async fn start(work_dir: std::path::PathBuf) -> Self {
        let cache = Arc::new(RwLock::new(None));

        // Initial collection
        if let Some(ctx) = collect_git_context(&work_dir).await {
            *cache.write().await = Some(ctx);
        }

        let clone_cache = cache.clone();
        let handle = tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(GIT_WATCHER_INTERVAL_SECS));
            loop {
                interval.tick().await;
                match collect_git_context(&work_dir).await {
                    Some(ctx) => {
                        *clone_cache.write().await = Some(ctx);
                    }
                    None => {
                        tracing::debug!(
                            "GitWatcher: no git context available (not in repo or git error)"
                        );
                    }
                }
            }
        });

        Self {
            cache,
            handle: Some(handle),
        }
    }
}

impl Drop for GitWatcher {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}
