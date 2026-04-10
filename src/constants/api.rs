pub const ANTHROPIC_VERSION: &str = "2023-06-01";
pub const CONTENT_TYPE_JSON: &str = "application/json";

/// MCP protocol version used in the initialize handshake.
pub const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

pub const DEFAULT_USER_AGENT: &str = concat!("kezen/", env!("CARGO_PKG_VERSION"));
pub const DEFAULT_ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";
pub const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com";

pub const DEFAULT_MAX_TOKENS: u32 = 128_000;

/// Maximum output tokens for compact operations.
pub const COMPACT_MAX_OUTPUT_TOKENS: u32 = 20_000;
