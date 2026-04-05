//! Session Audit Trail — JSONL-based recording of every AI operation.
//!
//! Each session produces an append-only `~/.kezen/sessions/<session_id>.jsonl`
//! file where every line is an independent JSON object representing one event
//! (user message, assistant response, tool call, tool result, permission
//! decision, etc.).
//!
//! Design goals:
//! - **Audit**: complete record of AI actions for post-session review.
//! - **Cloud-native**: JSONL lines are directly ingestible by ELK / SLS / Fluentd.
//! - **Crash-safe**: each line is flushed individually; partial last line on crash
//!   is the only data at risk.
//!
//! The audit logger is separate from operational logs (tracing) and API debug
//! logs (debug_logger). It records *what the AI did*, not *how the program ran*.

use serde::Serialize;
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;

use crate::constants::defaults::{AUDIT_MAX_OUTPUT_LENGTH, AUDIT_RETENTION_DAYS};

// ─── Event Types ─────────────────────────────────────────────────────────────

/// A single audit event, serialized as one JSONL line.
///
/// Uses `#[serde(tag = "type")]` so each variant produces a flat JSON object
/// with a `"type": "variant_name"` field — ideal for ELK index routing.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuditEvent {
    SessionStart {
        session_id: String,
        timestamp: String,
        model: String,
        cwd: String,
    },
    UserMessage {
        session_id: String,
        uuid: String,
        timestamp: String,
        content: String,
    },
    AssistantText {
        session_id: String,
        uuid: String,
        parent_uuid: String,
        timestamp: String,
        content: String,
        input_tokens: u64,
        output_tokens: u64,
        cost_usd: f64,
    },
    ToolCall {
        session_id: String,
        uuid: String,
        parent_uuid: String,
        timestamp: String,
        tool_name: String,
        tool_id: String,
        input: serde_json::Value,
    },
    ToolResult {
        session_id: String,
        uuid: String,
        parent_uuid: String,
        timestamp: String,
        tool_id: String,
        is_error: bool,
        output: String,
        truncated: bool,
    },
    PermissionDecision {
        session_id: String,
        uuid: String,
        timestamp: String,
        tool_name: String,
        decision: String,
        risk_level: String,
    },
    PermissionResponse {
        session_id: String,
        uuid: String,
        parent_uuid: String,
        timestamp: String,
        allowed: bool,
        always_allow: bool,
    },
    SessionEnd {
        session_id: String,
        timestamp: String,
        total_cost_usd: f64,
        total_input_tokens: u64,
        total_output_tokens: u64,
    },
}

// ─── Logger ──────────────────────────────────────────────────────────────────

/// Append-only JSONL audit logger for a single session.
///
/// Each call to [`log()`] serializes an [`AuditEvent`] to one JSON line and
/// flushes it immediately. The underlying file is opened with `append(true)`.
pub struct SessionAuditLogger {
    writer: tokio::io::BufWriter<tokio::fs::File>,
}

impl SessionAuditLogger {
    /// Create a new audit logger for the given session ID.
    ///
    /// The log file is created at `~/.kezen/sessions/<session_id>.jsonl`.
    /// The `sessions/` directory is created if it doesn't exist.
    pub async fn new(session_id: &str) -> anyhow::Result<Self> {
        let dir = Self::sessions_dir();
        tokio::fs::create_dir_all(&dir).await?;
        let path = dir.join(format!("{}.jsonl", session_id));
        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await?;
        Ok(Self {
            writer: tokio::io::BufWriter::new(file),
        })
    }

    /// Append a single audit event as one JSONL line.
    pub async fn log(&mut self, event: &AuditEvent) {
        let line = match serde_json::to_string(event) {
            Ok(json) => json,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to serialize audit event");
                return;
            }
        };
        // Write line + newline, then flush.
        // Errors are logged but not propagated — audit must never crash the engine.
        if let Err(e) = self.writer.write_all(line.as_bytes()).await {
            tracing::warn!(error = %e, "Failed to write audit event");
            return;
        }
        if let Err(e) = self.writer.write_all(b"\n").await {
            tracing::warn!(error = %e, "Failed to write audit newline");
            return;
        }
        if let Err(e) = self.writer.flush().await {
            tracing::warn!(error = %e, "Failed to flush audit log");
        }
    }

    /// Truncate tool output to [`MAX_OUTPUT_LENGTH`] if needed.
    pub fn truncate_output(output: &str) -> (String, bool) {
        if output.len() <= AUDIT_MAX_OUTPUT_LENGTH {
            (output.to_string(), false)
        } else {
            // Find a safe UTF-8 boundary
            let mut end = AUDIT_MAX_OUTPUT_LENGTH;
            while end > 0 && !output.is_char_boundary(end) {
                end -= 1;
            }
            (format!("{}...[truncated]", &output[..end]), true)
        }
    }

    /// Generate a new UUID for audit event linking.
    pub fn new_uuid() -> String {
        uuid::Uuid::new_v4().to_string()
    }

    /// Get the current timestamp in RFC 3339 format.
    pub fn now() -> String {
        chrono::Utc::now().to_rfc3339()
    }

    fn sessions_dir() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".kezen")
            .join("sessions")
    }
}

// ─── Cleanup ─────────────────────────────────────────────────────────────────

/// Delete `.jsonl` audit files older than [`RETENTION_DAYS`].
///
/// Called at startup to avoid unbounded disk growth. Errors on individual
/// files are logged and skipped — we never fail the startup over cleanup.
pub async fn cleanup_old_audit_logs() {
    let dir = SessionAuditLogger::sessions_dir();
    if !tokio::fs::try_exists(&dir).await.unwrap_or(false) {
        return;
    }
    let cutoff = std::time::SystemTime::now()
        - std::time::Duration::from_secs(AUDIT_RETENTION_DAYS * 24 * 3600);

    let mut entries = match tokio::fs::read_dir(&dir).await {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(error = %e, "Cannot read sessions dir for cleanup");
            return;
        }
    };

    let mut deleted = 0u32;
    let mut errors = 0u32;

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "jsonl")
            && let Ok(meta) = tokio::fs::metadata(&path).await
            && let Ok(modified) = meta.modified()
            && modified < cutoff
        {
            if tokio::fs::remove_file(&path).await.is_ok() {
                deleted += 1;
            } else {
                errors += 1;
            }
        }
    }

    if deleted > 0 || errors > 0 {
        tracing::info!(deleted, errors, "Audit log cleanup completed");
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_output_short() {
        let (output, truncated) = SessionAuditLogger::truncate_output("hello");
        assert_eq!(output, "hello");
        assert!(!truncated);
    }

    #[test]
    fn test_truncate_output_exact_limit() {
        let input = "a".repeat(AUDIT_MAX_OUTPUT_LENGTH);
        let (output, truncated) = SessionAuditLogger::truncate_output(&input);
        assert_eq!(output, input);
        assert!(!truncated);
    }

    #[test]
    fn test_truncate_output_over_limit() {
        let input = "a".repeat(AUDIT_MAX_OUTPUT_LENGTH + 100);
        let (output, truncated) = SessionAuditLogger::truncate_output(&input);
        assert!(truncated);
        assert!(output.ends_with("...[truncated]"));
        assert!(output.len() < input.len());
    }

    #[test]
    fn test_truncate_output_utf8_boundary() {
        // 'é' is 2 bytes in UTF-8
        let mut input = "é".repeat(AUDIT_MAX_OUTPUT_LENGTH);
        input.push_str("extra");
        let (output, truncated) = SessionAuditLogger::truncate_output(&input);
        assert!(truncated);
        // Must not panic or produce invalid UTF-8
        assert!(output.is_char_boundary(0));
    }

    #[test]
    fn test_audit_event_serialization() {
        let event = AuditEvent::SessionStart {
            session_id: "test-123".to_string(),
            timestamp: "2026-04-05T00:00:00Z".to_string(),
            model: "claude-3-5-sonnet".to_string(),
            cwd: "/project".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"session_start\""));
        assert!(json.contains("\"session_id\":\"test-123\""));
    }

    #[test]
    fn test_audit_event_tool_result_serialization() {
        let event = AuditEvent::ToolResult {
            session_id: "test-123".to_string(),
            uuid: "msg-001".to_string(),
            parent_uuid: "msg-000".to_string(),
            timestamp: "2026-04-05T00:00:00Z".to_string(),
            tool_id: "toolu_01".to_string(),
            is_error: false,
            output: "file contents".to_string(),
            truncated: false,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"tool_result\""));
        assert!(json.contains("\"truncated\":false"));
    }

    #[test]
    fn test_audit_event_permission_decision_serialization() {
        let event = AuditEvent::PermissionDecision {
            session_id: "test-123".to_string(),
            uuid: "msg-005".to_string(),
            timestamp: "2026-04-05T00:00:00Z".to_string(),
            tool_name: "Bash".to_string(),
            decision: "needs_approval".to_string(),
            risk_level: "medium".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"permission_decision\""));
        assert!(json.contains("\"decision\":\"needs_approval\""));
    }

    #[tokio::test]
    async fn test_logger_writes_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let session_id = "test-write";
        let path = dir.path().join(format!("{}.jsonl", session_id));

        // Manually create the logger pointing to temp dir
        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .unwrap();
        let mut logger = SessionAuditLogger {
            writer: tokio::io::BufWriter::new(file),
        };

        logger.log(&AuditEvent::SessionStart {
            session_id: session_id.to_string(),
            timestamp: "2026-04-05T00:00:00Z".to_string(),
            model: "test-model".to_string(),
            cwd: "/tmp".to_string(),
        }).await;

        logger.log(&AuditEvent::UserMessage {
            session_id: session_id.to_string(),
            uuid: "msg-001".to_string(),
            timestamp: "2026-04-05T00:00:01Z".to_string(),
            content: "hello".to_string(),
        }).await;

        // Read back and verify
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        let lines: Vec<&str> = content.trim().lines().collect();
        assert_eq!(lines.len(), 2);

        // Each line must be valid JSON
        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["type"], "session_start");
        assert_eq!(first["session_id"], "test-write");

        let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(second["type"], "user_message");
        assert_eq!(second["content"], "hello");
    }
}
