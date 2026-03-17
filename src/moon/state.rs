use crate::moon::paths::MoonPaths;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

pub const HOT_EMBED_COLLECTION_PREFIX: &str = "history_hot_";
pub const HOT_EMBED_COLLECTION_FALLBACK: &str = "history_hot";
pub const LIBRARY_EMBED_COLLECTION: &str = "history_lib";

pub type EmbeddedProjectionEntries = BTreeMap<String, u64>;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RawSessionCursor {
    pub bytes: u64,
    pub lines: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MoonState {
    pub schema_version: u32,
    pub last_heartbeat_epoch_secs: u64,
    pub last_archive_trigger_epoch_secs: Option<u64>,
    #[serde(alias = "last_prune_trigger_epoch_secs")]
    pub last_compaction_trigger_epoch_secs: Option<u64>,
    pub last_distill_trigger_epoch_secs: Option<u64>,
    pub last_syns_trigger_epoch_secs: Option<u64>,
    pub last_embed_trigger_epoch_secs: Option<u64>,
    pub last_session_id: Option<String>,
    pub last_usage_ratio: Option<f64>,
    pub last_provider: Option<String>,
    pub last_assembly_session_id: Option<String>,
    pub last_assembly_epoch_secs: Option<u64>,
    pub distilled_archives: BTreeMap<String, u64>,
    pub embedded_projection_collections: BTreeMap<String, EmbeddedProjectionEntries>,
    pub embedded_projections: BTreeMap<String, u64>,
    pub pending_embed_collections: BTreeMap<String, u64>,
    pub managed_hot_collections: BTreeMap<String, u64>,
    pub raw_session_cursors: BTreeMap<String, RawSessionCursor>,
    pub inbound_seen_files: BTreeMap<String, u64>,
}

impl Default for MoonState {
    fn default() -> Self {
        Self {
            schema_version: 6,
            last_heartbeat_epoch_secs: 0,
            last_archive_trigger_epoch_secs: None,
            last_compaction_trigger_epoch_secs: None,
            last_distill_trigger_epoch_secs: None,
            last_syns_trigger_epoch_secs: None,
            last_embed_trigger_epoch_secs: None,
            last_session_id: None,
            last_usage_ratio: None,
            last_provider: None,
            last_assembly_session_id: None,
            last_assembly_epoch_secs: None,
            distilled_archives: BTreeMap::new(),
            embedded_projection_collections: BTreeMap::new(),
            embedded_projections: BTreeMap::new(),
            pending_embed_collections: BTreeMap::new(),
            managed_hot_collections: BTreeMap::new(),
            raw_session_cursors: BTreeMap::new(),
            inbound_seen_files: BTreeMap::new(),
        }
    }
}

pub fn mark_embed_maintenance_pending(
    state: &mut MoonState,
    collection_name: &str,
    queued_at_epoch_secs: u64,
) {
    let collection = collection_name.trim();
    if collection.is_empty() {
        return;
    }
    state
        .pending_embed_collections
        .insert(collection.to_string(), queued_at_epoch_secs);
}

pub fn hot_embed_collection_for_session(session_id: &str) -> String {
    format!(
        "{}{}",
        HOT_EMBED_COLLECTION_PREFIX,
        sanitize_collection_segment(session_id)
    )
}

pub fn hot_projection_dir_for_collection(paths: &MoonPaths, collection_name: &str) -> PathBuf {
    paths.mds_dir.join(collection_name)
}

pub fn hot_projection_path_for_session(paths: &MoonPaths, session_id: &str) -> PathBuf {
    hot_projection_dir_for_collection(paths, &hot_embed_collection_for_session(session_id))
        .join("session.md")
}

pub fn is_hot_embed_collection(collection_name: &str) -> bool {
    let trimmed = collection_name.trim();
    trimmed == HOT_EMBED_COLLECTION_FALLBACK || trimmed.starts_with(HOT_EMBED_COLLECTION_PREFIX)
}

fn sanitize_collection_segment(raw: &str) -> String {
    let mut out = String::new();
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            continue;
        }
        if ch == '-' || ch == '_' {
            out.push(ch);
            continue;
        }
        out.push('-');
    }

    let normalized = out.trim_matches('-');
    if normalized.is_empty() {
        "session".to_string()
    } else {
        normalized.to_string()
    }
}

pub fn state_file_path(paths: &MoonPaths) -> PathBuf {
    if let Ok(custom_file) = env::var("MOON_STATE_FILE") {
        let trimmed = custom_file.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    if let Ok(custom_dir) = env::var("MOON_STATE_DIR") {
        let trimmed = custom_dir.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed).join("moon_state.json");
        }
    }
    paths.moon_home.join("state").join("moon_state.json")
}

fn infer_collection_for_projection_path(paths: &MoonPaths, path: &Path) -> String {
    if path.starts_with(&paths.mlib_dir) {
        return LIBRARY_EMBED_COLLECTION.to_string();
    }
    if path.starts_with(&paths.mds_dir) {
        if let Ok(relative) = path.strip_prefix(&paths.mds_dir)
            && let Some(component) = relative.components().next()
        {
            let candidate = component.as_os_str().to_string_lossy().trim().to_string();
            if is_hot_embed_collection(&candidate) {
                return candidate;
            }
        }
        return path
            .file_stem()
            .and_then(|value| value.to_str())
            .map(hot_embed_collection_for_session)
            .unwrap_or_else(|| HOT_EMBED_COLLECTION_FALLBACK.to_string());
    }
    LIBRARY_EMBED_COLLECTION.to_string()
}

pub fn rebuild_legacy_embedded_projection_index(state: &mut MoonState) {
    let mut flattened = BTreeMap::new();
    for entries in state.embedded_projection_collections.values() {
        for (path, epoch) in entries {
            flattened
                .entry(path.clone())
                .and_modify(|existing: &mut u64| *existing = (*existing).max(*epoch))
                .or_insert(*epoch);
        }
    }
    state.embedded_projections = flattened;
}

pub fn embedded_projection_epoch(
    state: &MoonState,
    collection_name: &str,
    path: &str,
) -> Option<u64> {
    state
        .embedded_projection_collections
        .get(collection_name)
        .and_then(|entries| entries.get(path))
        .copied()
}

pub fn count_embedded_projection_docs_under(
    state: &MoonState,
    collection_name: &str,
    root: &Path,
) -> usize {
    state
        .embedded_projection_collections
        .get(collection_name)
        .map(|entries| {
            entries
                .keys()
                .filter(|path| Path::new(path).starts_with(root))
                .count()
        })
        .unwrap_or(0)
}

pub fn record_embedded_projection(
    state: &mut MoonState,
    collection_name: &str,
    path: String,
    embedded_at_epoch_secs: u64,
) {
    state
        .embedded_projection_collections
        .entry(collection_name.to_string())
        .or_default()
        .insert(path, embedded_at_epoch_secs);
    rebuild_legacy_embedded_projection_index(state);
}

pub fn retain_embedded_projections_for_collection<F>(
    state: &mut MoonState,
    collection_name: &str,
    mut keep: F,
) where
    F: FnMut(&str, u64) -> bool,
{
    let mut remove_collection = false;
    if let Some(entries) = state
        .embedded_projection_collections
        .get_mut(collection_name)
    {
        entries.retain(|path, epoch| keep(path, *epoch));
        remove_collection = entries.is_empty();
    }
    if remove_collection {
        state
            .embedded_projection_collections
            .remove(collection_name);
    }
    rebuild_legacy_embedded_projection_index(state);
}

pub fn remove_embedded_projection_collection(state: &mut MoonState, collection_name: &str) {
    state
        .embedded_projection_collections
        .remove(collection_name);
    rebuild_legacy_embedded_projection_index(state);
}

pub fn remove_embedded_projection_paths(
    state: &mut MoonState,
    removed_paths: &std::collections::BTreeSet<String>,
) -> usize {
    let before_total = state
        .embedded_projection_collections
        .values()
        .map(BTreeMap::len)
        .sum::<usize>();

    state.embedded_projection_collections.retain(|_, entries| {
        entries.retain(|path, _| !removed_paths.contains(path));
        !entries.is_empty()
    });
    rebuild_legacy_embedded_projection_index(state);

    let after_total = state
        .embedded_projection_collections
        .values()
        .map(BTreeMap::len)
        .sum::<usize>();
    before_total.saturating_sub(after_total)
}

pub fn load(paths: &MoonPaths) -> Result<MoonState> {
    let file = state_file_path(paths);
    if !file.exists() {
        return Ok(MoonState::default());
    }

    let raw =
        fs::read_to_string(&file).with_context(|| format!("failed to read {}", file.display()))?;

    let mut parsed: MoonState = match serde_json::from_str(&raw) {
        Ok(s) => s,
        Err(err) => {
            let timestamp = crate::moon::util::now_epoch_secs().unwrap_or(0);
            let backup_path = file.with_extension(format!("json.corrupt.{}", timestamp));
            let _ = fs::write(&backup_path, &raw);

            crate::moon::warn::emit(crate::moon::warn::WarnEvent {
                code: "STATE_CORRUPT",
                stage: "startup",
                action: "load-state",
                session: "na",
                archive: "na",
                source: &file.display().to_string(),
                retry: "started-fresh",
                reason: "json-parse-failed",
                err: &format!("{err:#}"),
            });

            return Ok(MoonState::default());
        }
    };

    if parsed.embedded_projection_collections.is_empty() && !parsed.embedded_projections.is_empty()
    {
        for (path, epoch) in parsed.embedded_projections.clone() {
            let collection = infer_collection_for_projection_path(paths, Path::new(&path));
            parsed
                .embedded_projection_collections
                .entry(collection)
                .or_default()
                .insert(path, epoch);
        }
    }

    rebuild_legacy_embedded_projection_index(&mut parsed);

    if parsed.schema_version < 7 {
        parsed.schema_version = 7;
    }
    Ok(parsed)
}

pub fn save(paths: &MoonPaths, state: &MoonState) -> Result<PathBuf> {
    let file = state_file_path(paths);
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let data = serde_json::to_string_pretty(state)?;
    fs::write(&file, format!("{data}\n"))
        .with_context(|| format!("failed to write {}", file.display()))?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::MoonState;

    #[test]
    fn deserializes_v1_state_with_embed_defaults() {
        let raw = r#"{
  "schema_version": 1,
  "last_heartbeat_epoch_secs": 10,
  "distilled_archives": {}
}"#;
        let parsed: MoonState = serde_json::from_str(raw).expect("parse state");
        assert_eq!(parsed.schema_version, 1);
        assert!(parsed.last_embed_trigger_epoch_secs.is_none());
        assert!(parsed.last_assembly_session_id.is_none());
        assert!(parsed.last_assembly_epoch_secs.is_none());
        assert!(parsed.embedded_projection_collections.is_empty());
        assert!(parsed.embedded_projections.is_empty());
        assert!(parsed.pending_embed_collections.is_empty());
        assert!(parsed.managed_hot_collections.is_empty());
        assert!(parsed.raw_session_cursors.is_empty());
    }

    #[test]
    fn hot_collection_name_sanitizes_session_id() {
        let collection = super::hot_embed_collection_for_session("Agent:Main/Session 01");
        assert_eq!(collection, "history_hot_agent-main-session-01");
    }

    #[test]
    fn history_collection_is_not_treated_as_hot() {
        assert!(!super::is_hot_embed_collection("history"));
        assert!(super::is_hot_embed_collection("history_hot"));
        assert!(super::is_hot_embed_collection("history_hot_abc"));
    }
}
