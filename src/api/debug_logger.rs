use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use chrono::Local;
use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};

/// Global flag: when true, raw API I/O is logged to ~/.kezen/api_logs/
static DEBUG_ENABLED: AtomicBool = AtomicBool::new(false);

/// Non-blocking writer for API debug logs.
///
/// Using `OnceLock` guarantees the writer is initialised exactly once.
/// The `WorkerGuard` is kept alive for the entire process lifetime;
/// dropping it flushes any buffered data.
///
/// The `Mutex<NonBlocking>` is needed because `NonBlocking` only implements
/// `Write` via `&mut self`. Contention is negligible - API calls are sequential.
static API_LOG_WRITER: OnceLock<(Mutex<NonBlocking>, WorkerGuard)> = OnceLock::new();

/// Enable API debug logging. Call once at startup when --verbose is set.
///
/// Creates a timestamped log file under `~/.kezen/api_logs/`.
/// Uses `std::fs::create_dir_all` (blocking) because this runs once
/// at startup before any async I/O begins.
pub fn enable_debug_logging() {
    DEBUG_ENABLED.store(true, Ordering::Relaxed);
    // Eagerly initialise the writer so any filesystem errors surface early.
    API_LOG_WRITER.get_or_init(|| {
        let home = dirs::home_dir().expect("Cannot determine home directory");
        let log_dir = home.join(".kezen").join("api_logs");
        std::fs::create_dir_all(&log_dir).expect("Cannot create ~/.kezen/api_logs/");
        let timestamp = Local::now().format("%Y%m%d_%H%M%S");
        let file = std::fs::File::options()
            .create(true)
            .append(true)
            .open(log_dir.join(format!("api_debug_{}.log", timestamp)))
            .expect("Cannot create API debug log file");
        let (non_blocking, guard) = tracing_appender::non_blocking(file);
        (Mutex::new(non_blocking), guard)
    });
}

pub fn is_debug_enabled() -> bool {
    DEBUG_ENABLED.load(Ordering::Relaxed)
}

/// Write a formatted string to the API debug log file.
fn write_to_api_log(msg: &str) {
    if let Some((writer_mutex, _guard)) = API_LOG_WRITER.get() {
        if let Ok(mut writer) = writer_mutex.lock() {
            let _ = writeln!(writer, "{}", msg);
        }
    }
}

/// Log a raw API request body (pretty-printed JSON).
pub fn log_request(provider: &str, url: &str, body: &serde_json::Value) {
    if !is_debug_enabled() {
        return;
    }
    let pretty_body = serde_json::to_string_pretty(body).unwrap_or_else(|_| body.to_string());
    tracing::debug!(provider, url, bytes = pretty_body.len(), "API request");

    let timestamp = Local::now().format("%H:%M:%S%.3f");
    let separator = "=".repeat(80);
    write_to_api_log(&format!(
        "\n{}\n[{}] >>> REQUEST to {} ({})\n{}\n{}",
        separator, timestamp, provider, url, separator, pretty_body
    ));
}

/// Log a raw API error response.
pub fn log_error_response(provider: &str, status: u16, body: &str) {
    tracing::debug!(provider, status, "API error response");

    if !is_debug_enabled() {
        return;
    }
    let timestamp = Local::now().format("%H:%M:%S%.3f");
    write_to_api_log(&format!(
        "\n[{}] <<< ERROR from {} (HTTP {})\n{}",
        timestamp, provider, status, body
    ));
}

/// Log a single raw SSE event from the stream.
pub fn log_sse_event(provider: &str, event_type: &str, data: &str) {
    if !is_debug_enabled() {
        return;
    }
    let timestamp = Local::now().format("%H:%M:%S%.3f");
    write_to_api_log(&format!(
        "[{}] <<< SSE [{}] event={} data={}",
        timestamp, provider, event_type, data
    ));
}

/// Log the final summary after a stream completes.
pub fn log_stream_end(provider: &str, input_tokens: u64, output_tokens: u64) {
    tracing::debug!(provider, input_tokens, output_tokens, "Stream ended");

    if !is_debug_enabled() {
        return;
    }
    let timestamp = Local::now().format("%H:%M:%S%.3f");
    write_to_api_log(&format!(
        "[{}] --- STREAM END [{}] tokens: {} in / {} out\n",
        timestamp, provider, input_tokens, output_tokens
    ));
}
