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

// ── Skills ───────────────────────────────────────────────────────────────────

/// Canonical tool name for the Skill meta-tool.
pub const SKILL_TOOL_NAME: &str = "Skill";

/// Skill listing gets 1% of the context window (in characters).
/// Listing is for discovery only — full content is loaded on invocation.
#[allow(dead_code)] // TODO: Use for dynamic budget: context_window × CHARS_PER_TOKEN × PERCENT
pub const SKILL_BUDGET_CONTEXT_PERCENT: f64 = 0.01;

/// Assumed characters-per-token ratio for budget calculation.
#[allow(dead_code)] // TODO: Use for dynamic budget: context_window × CHARS_PER_TOKEN × PERCENT
pub const SKILL_CHARS_PER_TOKEN: usize = 4;

/// Fallback character budget when context window size is unknown.
/// Equivalent to 1% of 200k tokens × 4 chars/token = 8000.
pub const DEFAULT_SKILL_BUDGET_CHARS: usize = 8_000;

/// Per-entry hard cap for skill description in the listing.
/// Verbose `when_to_use` strings waste turn-1 cache tokens without
/// improving match rate; this keeps each entry concise.
pub const MAX_LISTING_DESC_CHARS: usize = 250;

// ── Context Budget & Truncation ──────────────────────────────────────────────

/// Maximum tokens for a single tool result stored in context
pub const MAX_TOOL_RESULT_CONTEXT_TOKENS: u64 = 50_000;

/// Background git context refresh interval (seconds)
pub const GIT_WATCHER_INTERVAL_SECS: u64 = 30;

// ── Channel Buffer Sizes ────────────────────────────────────────────────────

/// Buffer size for the `UserAction` mpsc channel (Frontend → Engine).
/// Uses backpressure: sender awaits if full.
pub const ACTION_CHANNEL_BUFFER: usize = 32;

/// Buffer size for the `EngineEvent` broadcast channel (Engine → Frontends).
/// No backpressure: oldest messages are overwritten when full (receivers get `Lagged`).
/// gRPC adapters should implement their own per-client buffering rather than
/// relying on this value.
pub const EVENT_CHANNEL_BUFFER: usize = 64;
