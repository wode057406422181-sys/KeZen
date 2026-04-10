// ─── UI Limits ───────────────────────────────────────────────────────────────
pub const UI_MAX_TEXT_CHARS: usize = 500;
pub const UI_MAX_THINKING_CHARS: usize = 100;
pub const UI_MAX_TOOL_INPUT_CHARS: usize = 80;
pub const UI_MAX_TOOL_RESULT_CHARS: usize = 200;
pub const UI_MAX_TOOL_RESULT_HISTORY_CHARS: usize = 100;

// ─── Context & Memory Limits ──────────────────────────────────────────────────
pub const MAX_TOOL_RESULT_CONTEXT_TOKENS: u64 = 50_000;
pub const MAX_GIT_STATUS_CHARS: usize = 1000;
pub const MAX_MEMORY_CHARACTER_COUNT: usize = 40_000;

// ─── Storage & Audit Limits ───────────────────────────────────────────────────
pub const AUDIT_MAX_OUTPUT_LENGTH: usize = 4096;
pub const AUDIT_RETENTION_DAYS: u64 = 30;
pub const WEB_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(15 * 60);
pub const WEB_CACHE_MAX_ENTRIES: usize = 100;

// ─── Web Fetch Limits ─────────────────────────────────────────────────────────
pub const FETCH_MAX_MARKDOWN_LENGTH: usize = 100_000;
pub const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
pub const FETCH_MAX_CONTENT_LENGTH: u64 = 10 * 1024 * 1024;
pub const FETCH_MAX_URL_LENGTH: usize = 2000;

// ─── Skills Limits ────────────────────────────────────────────────────────────
pub const MAX_LISTING_DESC_CHARS: usize = 250;
