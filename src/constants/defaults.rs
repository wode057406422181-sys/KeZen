pub const DEFAULT_MAX_TOKENS: u32 = 8192;
pub const DEFAULT_USER_AGENT: &str = concat!("kezen/", env!("CARGO_PKG_VERSION"));
pub const DEFAULT_ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";
pub const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com";

/// Maximum output tokens for compact operations.
pub const COMPACT_MAX_OUTPUT_TOKENS: u32 = 20_000;
