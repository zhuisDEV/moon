use crate::moon::paths::MoonPaths;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

pub const DAEMON_LOCK_FILE: &str = "moon-watch.daemon.lock";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonLockPayload {
    pub pid: u32,
    #[serde(default, alias = "start_time")]
    pub started_at_epoch_secs: u64,
    #[serde(default)]
    pub build_uuid: String,
    #[serde(default)]
    pub moon_home: String,
}

pub fn daemon_lock_path(paths: &MoonPaths) -> PathBuf {
    paths.logs_dir.join(DAEMON_LOCK_FILE)
}

pub fn parse_daemon_lock_payload(raw: &str) -> Option<DaemonLockPayload> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    serde_json::from_str::<DaemonLockPayload>(trimmed).ok()
}

pub fn read_daemon_lock_payload(paths: &MoonPaths) -> Result<Option<DaemonLockPayload>> {
    let lock_path = daemon_lock_path(paths);
    if !lock_path.exists() {
        return Ok(None);
    }

    let raw = fs::read_to_string(&lock_path)
        .with_context(|| format!("failed to read daemon lock {}", lock_path.display()))?;
    Ok(parse_daemon_lock_payload(&raw))
}

#[cfg(test)]
mod tests {
    use super::parse_daemon_lock_payload;

    #[test]
    fn parses_json_payload() {
        let raw = r#"{"pid":42,"started_at_epoch_secs":1700000000,"build_uuid":"abc","moon_home":"/tmp/moon"}"#;
        let payload = parse_daemon_lock_payload(raw).expect("payload");
        assert_eq!(payload.pid, 42);
        assert_eq!(payload.build_uuid, "abc");
    }
}
