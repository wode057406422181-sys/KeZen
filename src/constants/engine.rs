/// Buffer size for the `UserAction` mpsc channel (Frontend → Engine).
pub const ACTION_CHANNEL_BUFFER: usize = 32;

/// Buffer size for the `EngineEvent` broadcast channel (Engine → Frontends).
pub const EVENT_CHANNEL_BUFFER: usize = 64;

/// Maximum iterations of the agentic loop per user message.
pub const MAX_AGENTIC_LOOP_ITERATIONS: usize = 200;

/// Background git context refresh interval (seconds)
pub const GIT_WATCHER_INTERVAL_SECS: u64 = 30;

// ─── Skills Configuration ─────────────────────────────────────────────────────

/// Canonical tool name for the Skill meta-tool.
pub const SKILL_TOOL_NAME: &str = "Skill";

#[allow(dead_code)]
pub const SKILL_BUDGET_CONTEXT_PERCENT: f64 = 0.01;
#[allow(dead_code)]
pub const SKILL_CHARS_PER_TOKEN: usize = 4;

/// Fallback character budget when context window size is unknown.
pub const DEFAULT_SKILL_BUDGET_CHARS: usize = 8_000;
