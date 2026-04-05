pub const DEFAULT_MAX_TOKENS: u32 = 128_000;
pub const DEFAULT_USER_AGENT: &str = concat!("kezen/", env!("CARGO_PKG_VERSION"));
pub const DEFAULT_ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";
pub const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com";

/// Maximum output tokens for compact operations.
pub const COMPACT_MAX_OUTPUT_TOKENS: u32 = 20_000;

// ── Audit ────────────────────────────────────────────────────────────────────

/// Maximum length for tool output in audit trail events (bytes).
pub const AUDIT_MAX_OUTPUT_LENGTH: usize = 4096;

/// Number of days to retain audit log files before auto-cleanup.
pub const AUDIT_RETENTION_DAYS: u64 = 30;

// ── Web Fetch ────────────────────────────────────────────────────────────────

/// Maximum markdown content length returned by WebFetch (characters).
pub const FETCH_MAX_MARKDOWN_LENGTH: usize = 100_000;

/// HTTP request timeout for WebFetch.
pub const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Maximum HTTP response body size for WebFetch (bytes).
pub const FETCH_MAX_CONTENT_LENGTH: u64 = 10 * 1024 * 1024;

/// Maximum URL length accepted by WebFetch.
pub const FETCH_MAX_URL_LENGTH: usize = 2000;

// ── Web Cache ────────────────────────────────────────────────────────────────

/// TTL for cached web page entries.
pub const WEB_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// Maximum number of cached web page entries.
pub const WEB_CACHE_MAX_ENTRIES: usize = 100;

// ── Memory ───────────────────────────────────────────────────────────────────

/// Maximum character count for a single memory file before truncation.
pub const MAX_MEMORY_CHARACTER_COUNT: usize = 40_000;
