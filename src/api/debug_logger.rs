use std::fs::{self, File};
use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::Local;

/// Global flag: when true, raw API I/O is logged to ~/.kezen/logs/
static DEBUG_ENABLED: AtomicBool = AtomicBool::new(false);

/// Cached open log file handle — opened once per process, reused for every write.
///
/// Thread-safe: the `Mutex<File>` serializes concurrent writes; `OnceLock`
/// ensures the file is created exactly once.
static LOG_FILE: OnceLock<Mutex<File>> = OnceLock::new();

/// Enable API debug logging. Call once at startup when --verbose is set.
pub fn enable_debug_logging() {
    DEBUG_ENABLED.store(true, Ordering::Relaxed);
}

pub fn is_debug_enabled() -> bool {
    DEBUG_ENABLED.load(Ordering::Relaxed)
}

/// Get or initialise the log file for this session, returning a locked guard.
///
/// Returns `None` if the home directory cannot be determined or the file
/// cannot be created — in which case logging is silently skipped.
fn with_log_file(f: impl FnOnce(&mut File)) {
    if !is_debug_enabled() {
        return;
    }

    let file_mutex = LOG_FILE.get_or_init(|| {
        let path = (|| -> Option<std::path::PathBuf> {
            let home = dirs::home_dir()?;
            let log_dir = home.join(".kezen").join("logs");
            fs::create_dir_all(&log_dir).ok()?;
            let timestamp = Local::now().format("%Y%m%d_%H%M%S");
            Some(log_dir.join(format!("api_debug_{}.log", timestamp)))
        })();

        match path.and_then(|p| {
            File::options()
                .create(true)
                .append(true)
                .open(p)
                .ok()
        }) {
            Some(file) => Mutex::new(file),
            // Fallback: open /dev/null so the OnceLock is always initialised.
            // Subsequent writes will silently succeed but go nowhere.
            None => Mutex::new(File::options().write(true).open("/dev/null").unwrap()),
        }
    });

    if let Ok(mut guard) = file_mutex.lock() {
        f(&mut guard);
    }
}

/// Log a raw API request body (pretty-printed JSON).
pub fn log_request(provider: &str, url: &str, body: &serde_json::Value) {
    let pretty_body = serde_json::to_string_pretty(body).unwrap_or_else(|_| body.to_string());
    tracing::debug!(provider, url, bytes = pretty_body.len(), "API request");

    with_log_file(|file| {
        let timestamp = Local::now().format("%H:%M:%S%.3f");
        let separator = "=".repeat(80);
        let _ = writeln!(
            file,
            "\n{}\n[{}] >>> REQUEST to {} ({})\n{}\n{}",
            separator, timestamp, provider, url, separator, pretty_body
        );
    });
}

/// Log a raw API error response.
pub fn log_error_response(provider: &str, status: u16, body: &str) {
    tracing::debug!(provider, status, "API error response");

    with_log_file(|file| {
        let timestamp = Local::now().format("%H:%M:%S%.3f");
        let _ = writeln!(
            file,
            "\n[{}] <<< ERROR from {} (HTTP {})\n{}",
            timestamp, provider, status, body
        );
    });
}

/// Log a single raw SSE event from the stream.
pub fn log_sse_event(provider: &str, event_type: &str, data: &str) {
    with_log_file(|file| {
        let timestamp = Local::now().format("%H:%M:%S%.3f");
        let _ = writeln!(
            file,
            "[{}] <<< SSE [{}] event={} data={}",
            timestamp, provider, event_type, data
        );
    });
}

/// Log the final summary after a stream completes.
pub fn log_stream_end(provider: &str, input_tokens: u64, output_tokens: u64) {
    tracing::debug!(provider, input_tokens, output_tokens, "Stream ended");

    with_log_file(|file| {
        let timestamp = Local::now().format("%H:%M:%S%.3f");
        let _ = writeln!(
            file,
            "[{}] --- STREAM END [{}] tokens: {} in / {} out\n",
            timestamp, provider, input_tokens, output_tokens
        );
    });
}
