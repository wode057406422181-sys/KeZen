use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub id: String,
    pub created_at: String,
    pub updated_at: String,
    pub messages: Vec<crate::api::types::Message>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
}

pub fn get_sessions_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".infini")
        .join("sessions")
}

pub async fn save_snapshot(snap: &SessionSnapshot) -> anyhow::Result<()> {
    let dir = get_sessions_dir();
    tokio::fs::create_dir_all(&dir).await?;
    let p = dir.join(format!("{}.json", snap.id));
    let json = serde_json::to_string_pretty(snap)?;
    tokio::fs::write(p, json).await?;
    Ok(())
}

pub async fn list_sessions() -> anyhow::Result<Vec<SessionSnapshot>> {
    let dir = get_sessions_dir();
    if !tokio::fs::try_exists(&dir).await.unwrap_or(false) {
        return Ok(vec![]);
    }
    let mut snaps = Vec::new();
    let mut entries = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        if entry.path().extension().is_some_and(|e| e == "json")
            && let Ok(c) = tokio::fs::read_to_string(entry.path()).await
                && let Ok(snap) = serde_json::from_str::<SessionSnapshot>(&c) {
                    snaps.push(snap);
                }
    }
    snaps.sort_by(|a, b| b.updated_at.cmp(&a.updated_at)); // Descending
    Ok(snaps)
}

pub async fn load_latest_snapshot() -> anyhow::Result<SessionSnapshot> {
    let snaps = list_sessions().await?;
    snaps
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("No previous session found"))
}

pub async fn load_snapshot_by_id(id: &str) -> anyhow::Result<SessionSnapshot> {
    let p = get_sessions_dir().join(format!("{}.json", id));
    let c = tokio::fs::read_to_string(p).await?;
    Ok(serde_json::from_str(&c)?)
}

pub async fn load_snapshot(id: Option<&str>) -> anyhow::Result<SessionSnapshot> {
    if let Some(target) = id {
        load_snapshot_by_id(target).await
    } else {
        load_latest_snapshot().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_snapshot(id: &str, updated_at: &str) -> SessionSnapshot {
        SessionSnapshot {
            id: id.to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: updated_at.to_string(),
            messages: vec![],
            input_tokens: 100,
            output_tokens: 50,
            cost_usd: 0.001,
        }
    }

    /// Override the sessions dir to use a temp directory for test isolation.
    fn save_to_dir(dir: &std::path::Path, snap: &SessionSnapshot) {
        std::fs::create_dir_all(dir).unwrap();
        let p = dir.join(format!("{}.json", snap.id));
        let json = serde_json::to_string_pretty(snap).unwrap();
        std::fs::write(p, json).unwrap();
    }

    fn load_from_dir(dir: &std::path::Path, id: &str) -> anyhow::Result<SessionSnapshot> {
        let p = dir.join(format!("{}.json", id));
        let c = std::fs::read_to_string(p)?;
        Ok(serde_json::from_str(&c)?)
    }

    fn list_from_dir(dir: &std::path::Path) -> anyhow::Result<Vec<SessionSnapshot>> {
        if !dir.exists() {
            return Ok(vec![]);
        }
        let mut snaps = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            if entry.path().extension().is_some_and(|e| e == "json") {
                if let Ok(c) = std::fs::read_to_string(entry.path()) {
                    if let Ok(snap) = serde_json::from_str::<SessionSnapshot>(&c) {
                        snaps.push(snap);
                    }
                }
            }
        }
        snaps.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(snaps)
    }

    #[test]
    fn test_snapshot_serialization_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let snap = make_snapshot("test-id-123", "2026-04-01T12:00:00Z");
        save_to_dir(dir.path(), &snap);

        let loaded = load_from_dir(dir.path(), "test-id-123").unwrap();
        assert_eq!(loaded.id, "test-id-123");
        assert_eq!(loaded.input_tokens, 100);
        assert_eq!(loaded.output_tokens, 50);
    }

    #[test]
    fn test_list_sessions_sorted_by_updated_at() {
        let dir = tempfile::tempdir().unwrap();
        save_to_dir(dir.path(), &make_snapshot("older", "2026-01-01T00:00:00Z"));
        save_to_dir(dir.path(), &make_snapshot("newer", "2026-04-01T00:00:00Z"));

        let sessions = list_from_dir(dir.path()).unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].id, "newer"); // Most recent first
        assert_eq!(sessions[1].id, "older");
    }

    #[test]
    fn test_list_sessions_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = list_from_dir(dir.path()).unwrap();
        assert!(sessions.is_empty());
    }

    #[test]
    fn test_load_nonexistent_id() {
        let dir = tempfile::tempdir().unwrap();
        let result = load_from_dir(dir.path(), "nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_snapshot_fields_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let snap = SessionSnapshot {
            id: "field-test".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-04-03T12:00:00Z".to_string(),
            messages: vec![
                crate::api::types::Message {
                    role: crate::api::types::Role::User,
                    content: vec![crate::api::types::ContentBlock::Text { text: "hello".into() }],
                },
            ],
            input_tokens: 500,
            output_tokens: 200,
            cost_usd: 0.0045,
        };
        save_to_dir(dir.path(), &snap);

        let loaded = load_from_dir(dir.path(), "field-test").unwrap();
        assert_eq!(loaded.messages.len(), 1);
        assert_eq!(loaded.cost_usd, 0.0045);
    }
}
