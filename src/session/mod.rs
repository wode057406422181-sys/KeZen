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
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("infini")
        .join("sessions")
}

pub fn save_snapshot(snap: &SessionSnapshot) -> anyhow::Result<()> {
    let dir = get_sessions_dir();
    std::fs::create_dir_all(&dir)?;
    let p = dir.join(format!("{}.json", snap.id));
    let json = serde_json::to_string_pretty(snap)?;
    std::fs::write(p, json)?;
    Ok(())
}

pub fn list_sessions() -> anyhow::Result<Vec<SessionSnapshot>> {
    let dir = get_sessions_dir();
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut snaps = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.path().extension().is_some_and(|e| e == "json")
            && let Ok(c) = std::fs::read_to_string(entry.path())
            && let Ok(snap) = serde_json::from_str::<SessionSnapshot>(&c) {
                snaps.push(snap);
            }
    }
    snaps.sort_by(|a, b| b.updated_at.cmp(&a.updated_at)); // Descending
    Ok(snaps)
}

pub fn load_latest_snapshot() -> anyhow::Result<SessionSnapshot> {
    let snaps = list_sessions()?;
    snaps
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("No previous session found"))
}

pub fn load_snapshot_by_id(id: &str) -> anyhow::Result<SessionSnapshot> {
    let p = get_sessions_dir().join(format!("{}.json", id));
    let c = std::fs::read_to_string(p)?;
    Ok(serde_json::from_str(&c)?)
}

pub fn load_snapshot(id: Option<&str>) -> anyhow::Result<SessionSnapshot> {
    if let Some(target) = id {
        load_snapshot_by_id(target)
    } else {
        load_latest_snapshot()
    }
}
