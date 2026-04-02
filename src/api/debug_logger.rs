use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::Local;

/// Global flag: when true, raw API I/O is logged to ~/.infini/logs/
static DEBUG_ENABLED: AtomicBool = AtomicBool::new(false);

/// Current session log file (shared across calls within one process)
static LOG_FILE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Enable API debug logging. Call once at startup when --verbose is set.
pub fn enable_debug_logging() {
    DEBUG_ENABLED.store(true, Ordering::Relaxed);
}

pub fn is_debug_enabled() -> bool {
    DEBUG_ENABLED.load(Ordering::Relaxed)
}

/// Get or create the log file path for this session.
fn get_log_path() -> Option<PathBuf> {
    let mut guard = LOG_FILE.lock().ok()?;
    if let Some(ref path) = *guard {
        return Some(path.clone());
    }

    let home = dirs::home_dir()?;
    let log_dir = home.join(".infini").join("logs");
    fs::create_dir_all(&log_dir).ok()?;

    let timestamp = Local::now().format("%Y%m%d_%H%M%S");
    let path = log_dir.join(format!("api_debug_{}.log", timestamp));
    *guard = Some(path.clone());
    Some(path)
}

/// Log a raw API request body (pretty-printed JSON).
pub fn log_request(provider: &str, url: &str, body: &serde_json::Value) {
    if !is_debug_enabled() {
        return;
    }
    let Some(path) = get_log_path() else {
        return;
    };
    let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };

    let timestamp = Local::now().format("%H:%M:%S%.3f");
    let pretty_body = serde_json::to_string_pretty(body).unwrap_or_else(|_| body.to_string());

    let separator = "=".repeat(80);
    let _ = writeln!(
        file,
        "\n{}\n[{}] >>> REQUEST to {} ({})\n{}\n{}",
        separator, timestamp, provider, url, separator, pretty_body
    );

    // Also print to stderr for immediate visibility
    eprintln!(
        "  🔍 [DEBUG] API request to {} ({} bytes)",
        url,
        pretty_body.len()
    );
}

/// Log a raw API error response.
pub fn log_error_response(provider: &str, status: u16, body: &str) {
    if !is_debug_enabled() {
        return;
    }
    let Some(path) = get_log_path() else {
        return;
    };
    let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };

    let timestamp = Local::now().format("%H:%M:%S%.3f");
    let _ = writeln!(
        file,
        "\n[{}] <<< ERROR from {} (HTTP {})\n{}",
        timestamp, provider, status, body
    );
}

/// Log a single raw SSE event from the stream.
pub fn log_sse_event(provider: &str, event_type: &str, data: &str) {
    if !is_debug_enabled() {
        return;
    }
    let Some(path) = get_log_path() else {
        return;
    };
    let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };

    let timestamp = Local::now().format("%H:%M:%S%.3f");
    let _ = writeln!(
        file,
        "[{}] <<< SSE [{}] event={} data={}",
        timestamp, provider, event_type, data
    );
}

/// Log the final summary after a stream completes.
pub fn log_stream_end(provider: &str, input_tokens: u32, output_tokens: u32) {
    if !is_debug_enabled() {
        return;
    }
    let Some(path) = get_log_path() else {
        return;
    };
    let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };

    let timestamp = Local::now().format("%H:%M:%S%.3f");
    let _ = writeln!(
        file,
        "[{}] --- STREAM END [{}] tokens: {} in / {} out\n",
        timestamp, provider, input_tokens, output_tokens
    );
}
