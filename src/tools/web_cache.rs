use std::sync::Mutex;
use std::time::Instant;

use lru::LruCache;
use std::num::NonZeroUsize;

use crate::constants::defaults::{WEB_CACHE_TTL, WEB_CACHE_MAX_ENTRIES};



/// A single cached web page result.
#[derive(Clone)]
pub struct CacheEntry {
    /// The converted markdown content.
    pub content: String,
    /// The original content-type header.
    pub content_type: String,
    /// The HTTP status code.
    pub status: u16,
    /// When this entry was inserted.
    fetched_at: Instant,
}

impl CacheEntry {
    fn is_expired(&self) -> bool {
        self.fetched_at.elapsed() > WEB_CACHE_TTL
    }
}

/// Thread-safe LRU cache for fetched web pages.
///
/// # Why `std::sync::Mutex` instead of `tokio::sync::Mutex`
///
/// We deliberately use `std::sync::Mutex` here because:
/// 1. The lock guard is **never held across an `.await` point** — all
///    operations (`get`, `insert`, `clear`) acquire the lock, do an
///    O(1) in-memory `LruCache` operation, and release immediately.
/// 2. `std::sync::Mutex` has lower overhead than its tokio counterpart
///    when the critical section is sub-microsecond (no async yield).
/// 3. `unwrap_or_else(|e| e.into_inner())` recovers from poisoned locks
///    so a panic in one task doesn't permanently brick the cache.
///
/// If future modifications ever need to hold the lock across an `.await`,
/// this MUST be changed to `tokio::sync::Mutex`.
pub struct WebCache {
    inner: Mutex<LruCache<String, CacheEntry>>,
}

impl WebCache {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(LruCache::new(
                NonZeroUsize::new(WEB_CACHE_MAX_ENTRIES).unwrap(),
            )),
        }
    }

    /// Look up a URL in the cache. Returns `None` if missing or expired.
    pub fn get(&self, url: &str) -> Option<CacheEntry> {
        let mut cache = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = cache.get(url) {
            if entry.is_expired() {
                cache.pop(url);
                return None;
            }
            return Some(entry.clone());
        }
        None
    }

    /// Insert a new entry into the cache.
    pub fn insert(&self, url: String, content: String, content_type: String, status: u16) {
        let entry = CacheEntry {
            content,
            content_type,
            status,
            fetched_at: Instant::now(),
        };
        let mut cache = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        cache.put(url, entry);
    }

    /// Clear all entries.
    #[allow(dead_code)] // TODO: Expose via /clear-cache command
    pub fn clear(&self) {
        let mut cache = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        cache.clear();
    }
}

impl Default for WebCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Global singleton cache, lazily initialised.
static GLOBAL_CACHE: std::sync::LazyLock<WebCache> = std::sync::LazyLock::new(WebCache::new);

/// Get the global web cache instance.
pub fn global_cache() -> &'static WebCache {
    &GLOBAL_CACHE
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_insert_and_get() {
        let cache = WebCache::new();
        cache.insert(
            "https://example.com".into(),
            "# Hello".into(),
            "text/html".into(),
            200,
        );
        let entry = cache.get("https://example.com").unwrap();
        assert_eq!(entry.content, "# Hello");
        assert_eq!(entry.status, 200);
    }

    #[test]
    fn test_cache_miss() {
        let cache = WebCache::new();
        assert!(cache.get("https://missing.com").is_none());
    }

    #[test]
    fn test_clear() {
        let cache = WebCache::new();
        cache.insert("https://a.com".into(), "a".into(), "text/html".into(), 200);
        cache.clear();
        assert!(cache.get("https://a.com").is_none());
    }

    #[test]
    fn test_lru_eviction() {
        // Create a cache with only 2 slots to test eviction
        let cache = WebCache {
            inner: Mutex::new(LruCache::new(NonZeroUsize::new(2).unwrap())),
        };

        cache.insert("https://a.com".into(), "a".into(), "text/html".into(), 200);
        cache.insert("https://b.com".into(), "b".into(), "text/html".into(), 200);
        // This should evict "a"
        cache.insert("https://c.com".into(), "c".into(), "text/html".into(), 200);

        assert!(cache.get("https://a.com").is_none());
        assert!(cache.get("https://b.com").is_some());
        assert!(cache.get("https://c.com").is_some());
    }

    #[test]
    fn test_concurrent_access() {
        let cache = std::sync::Arc::new(WebCache::new());

        let mut handles = vec![];
        for i in 0..10 {
            let c = cache.clone();
            handles.push(thread::spawn(move || {
                let url = format!("https://example.com/{}", i);
                c.insert(url.clone(), format!("content-{}", i), "text/html".into(), 200);
                assert!(c.get(&url).is_some());
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
    }

    #[test]
    fn test_expired_entry_returns_none() {
        // Create a cache and manually insert an already-expired entry
        let cache = WebCache::new();
        {
            let entry = CacheEntry {
                content: "old content".into(),
                content_type: "text/html".into(),
                status: 200,
                // Created 20 minutes ago → expired (TTL is 15 min)
                fetched_at: Instant::now() - Duration::from_secs(20 * 60),
            };
            let mut inner = cache.inner.lock().unwrap();
            inner.put("https://expired.com".into(), entry);
        }
        // Should return None since it's expired
        assert!(cache.get("https://expired.com").is_none());
    }

    #[test]
    fn test_fresh_entry_not_expired() {
        let entry = CacheEntry {
            content: "fresh".into(),
            content_type: "text/html".into(),
            status: 200,
            fetched_at: Instant::now(),
        };
        assert!(!entry.is_expired());
    }

    #[test]
    fn test_overwrite_existing_key() {
        let cache = WebCache::new();
        cache.insert("https://a.com".into(), "v1".into(), "text/html".into(), 200);
        cache.insert("https://a.com".into(), "v2".into(), "text/html".into(), 200);
        let entry = cache.get("https://a.com").unwrap();
        assert_eq!(entry.content, "v2");
    }

    #[test]
    fn test_cache_entry_preserves_status_and_content_type() {
        let cache = WebCache::new();
        cache.insert("https://a.com".into(), "body".into(), "application/json".into(), 404);
        let entry = cache.get("https://a.com").unwrap();
        assert_eq!(entry.content_type, "application/json");
        assert_eq!(entry.status, 404);
    }

    #[test]
    fn test_global_cache_returns_same_instance() {
        let c1 = global_cache();
        let c2 = global_cache();
        // Same pointer
        assert!(std::ptr::eq(c1, c2));
    }

    #[test]
    fn test_default_impl() {
        let cache = WebCache::default();
        assert!(cache.get("https://nonexistent.com").is_none());
    }
}
