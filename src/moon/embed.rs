use crate::moon::config::MoonEmbedConfig;
use crate::moon::files::{file_epoch_secs, gather_files_with_extension};
use crate::moon::paths::MoonPaths;
use crate::moon::qmd;
use crate::moon::state::{
    LIBRARY_EMBED_COLLECTION, MoonState, embedded_projection_epoch,
    hot_projection_dir_for_collection, is_hot_embed_collection, record_embedded_projection,
    retain_embedded_projections_for_collection,
};
use crate::moon::util::now_epoch_secs;
use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs;
use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;
use thiserror::Error;

const EMBED_LOCK_STALE_TTL_SECS: u64 = 21_600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbedCaller {
    Manual,
    Watcher,
}

impl EmbedCaller {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Watcher => "watcher",
        }
    }
}

#[derive(Debug, Clone)]
pub struct EmbedRunOptions {
    pub collection_name: String,
    pub max_docs: usize,
    pub dry_run: bool,
    pub caller: EmbedCaller,
    pub max_cycle_secs: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct EmbedRunSummary {
    pub collection: String,
    pub mode: String,
    pub capability: String,
    pub requested_max_docs: usize,
    pub selected_docs: usize,
    pub embedded_docs: usize,
    pub pending_before: usize,
    pub pending_after: usize,
    pub elapsed_ms: u128,
    pub degraded: bool,
    pub skip_reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SkipReason {
    None,
    Locked,
    CapabilityMissing,
    Cooldown,
}

impl SkipReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Locked => "locked",
            Self::CapabilityMissing => "capability-missing",
            Self::Cooldown => "cooldown",
        }
    }
}

#[derive(Debug, Error)]
pub enum EmbedRunError {
    #[error("embed capability missing: {0}")]
    CapabilityMissing(String),
    #[error("embed lock active: {0}")]
    Locked(String),
    #[error("embed status failed: {0}")]
    StatusFailed(String),
    #[error("embed failed: {0}")]
    Failed(String),
}

#[derive(Debug, Clone)]
struct ProjectionDoc {
    path: PathBuf,
    mtime_epoch_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EmbedLockPayload {
    pid: u32,
    started_at_epoch_secs: u64,
    mode: String,
    collection: String,
}

struct EmbedLockGuard {
    _file: fs::File,
}

fn is_cooldown_ready(last_epoch: Option<u64>, now_epoch: u64, cooldown_secs: u64) -> bool {
    match last_epoch {
        None => true,
        Some(last) => now_epoch.saturating_sub(last) >= cooldown_secs,
    }
}

fn projection_root_for_collection(paths: &MoonPaths, collection_name: &str) -> PathBuf {
    if collection_name.trim() == LIBRARY_EMBED_COLLECTION {
        return paths.mlib_dir.clone();
    }
    if is_hot_embed_collection(collection_name) {
        return hot_projection_dir_for_collection(paths, collection_name);
    }
    paths.mlib_dir.clone()
}

fn gather_projection_docs(root: &Path, out: &mut Vec<ProjectionDoc>) -> Result<()> {
    let mut paths = Vec::new();
    gather_files_with_extension(root, "md", true, &mut paths)?;
    for path in paths {
        out.push(ProjectionDoc {
            mtime_epoch_secs: file_epoch_secs(&path),
            path,
        });
    }

    Ok(())
}

fn projection_docs(root: &Path) -> Result<Vec<ProjectionDoc>> {
    let mut docs = Vec::new();
    gather_projection_docs(root, &mut docs)?;
    docs.sort_by(|a, b| {
        a.mtime_epoch_secs
            .cmp(&b.mtime_epoch_secs)
            .then_with(|| a.path.cmp(&b.path))
    });
    Ok(docs)
}

fn pending_docs<'a>(
    state: &MoonState,
    collection_name: &str,
    docs: &'a [ProjectionDoc],
) -> Vec<&'a ProjectionDoc> {
    docs.iter()
        .filter(|doc| {
            let key = doc.path.display().to_string();
            match embedded_projection_epoch(state, collection_name, &key) {
                None => true,
                Some(last_embed) => doc.mtime_epoch_secs > last_embed,
            }
        })
        .collect()
}

fn pid_alive(pid: u32) -> bool {
    crate::moon::util::pid_alive(pid)
}

fn read_lock_payload(lock_path: &Path) -> Option<EmbedLockPayload> {
    let raw = fs::read_to_string(lock_path).ok()?;
    serde_json::from_str::<EmbedLockPayload>(&raw).ok()
}

fn write_lock_payload(
    lock_file: &mut fs::File,
    mode: EmbedCaller,
    collection_name: &str,
    now_epoch: u64,
) -> Result<()> {
    let payload = EmbedLockPayload {
        pid: std::process::id(),
        started_at_epoch_secs: now_epoch,
        mode: mode.as_str().to_string(),
        collection: collection_name.to_string(),
    };

    lock_file.set_len(0)?;
    lock_file.write_all(format!("{}\n", serde_json::to_string(&payload)?).as_bytes())?;
    lock_file.flush()?;
    Ok(())
}

fn lock_is_stale(payload: &EmbedLockPayload, now_epoch: u64) -> bool {
    if !pid_alive(payload.pid) {
        return true;
    }
    now_epoch.saturating_sub(payload.started_at_epoch_secs) > EMBED_LOCK_STALE_TTL_SECS
}

fn acquire_lock(
    paths: &MoonPaths,
    mode: EmbedCaller,
    collection_name: &str,
    now_epoch: u64,
) -> Result<Option<EmbedLockGuard>> {
    crate::moon::fs_security::ensure_private_dir(&paths.logs_dir)?;
    let lock_path = paths.logs_dir.join("moon-embed.lock");

    let mut lock_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("failed to open embed lock {}", lock_path.display()))?;

    match lock_file.try_lock_exclusive() {
        Ok(()) => {
            write_lock_payload(&mut lock_file, mode, collection_name, now_epoch)?;
            Ok(Some(EmbedLockGuard { _file: lock_file }))
        }
        Err(err) if err.kind() == ErrorKind::WouldBlock => {
            let _ = read_lock_payload(&lock_path)
                .map(|payload| lock_is_stale(&payload, now_epoch))
                .unwrap_or(false);
            Ok(None)
        }
        Err(err) => Err(err).with_context(|| format!("failed to lock {}", lock_path.display())),
    }
}

fn is_embed_timeout(err: &anyhow::Error) -> bool {
    format!("{err:#}").contains("command timed out after")
}

fn run_bounded_embed_with_backoff(
    paths: &MoonPaths,
    opts: &EmbedRunOptions,
    initial_max_docs: usize,
) -> std::result::Result<(usize, qmd::EmbedExecResult), EmbedRunError> {
    let mut max_docs = initial_max_docs.max(1);
    loop {
        match qmd::embed_bounded(
            &paths.qmd_bin,
            &opts.collection_name,
            max_docs,
            opts.max_cycle_secs,
        ) {
            Ok(exec) => return Ok((max_docs, exec)),
            Err(err) => {
                if opts.caller == EmbedCaller::Watcher && is_embed_timeout(&err) && max_docs > 1 {
                    max_docs = (max_docs / 2).max(1);
                    continue;
                }
                let timeout_text = opts
                    .max_cycle_secs
                    .map(|secs| secs.to_string())
                    .unwrap_or_else(|| "none".to_string());
                return Err(EmbedRunError::Failed(format!(
                    "bounded-embed-failed max_docs={max_docs} timeout_secs={timeout_text} error={err:#}"
                )));
            }
        }
    }
}

fn infer_doc_collection(paths: &MoonPaths, path: &Path) -> String {
    if path.starts_with(&paths.mlib_dir) {
        return LIBRARY_EMBED_COLLECTION.to_string();
    }
    if let Ok(relative) = path.strip_prefix(&paths.mds_dir)
        && let Some(component) = relative.components().next()
    {
        let candidate = component.as_os_str().to_string_lossy().trim().to_string();
        if is_hot_embed_collection(&candidate) {
            return candidate;
        }
    }
    LIBRARY_EMBED_COLLECTION.to_string()
}

fn all_projection_docs(paths: &MoonPaths) -> Result<Vec<ProjectionDoc>> {
    let mut docs = projection_docs(&paths.mlib_dir)?;
    let mut hot_docs = projection_docs(&paths.mds_dir)?;
    docs.append(&mut hot_docs);
    Ok(docs)
}

fn reconcile_embedded_projection_state(paths: &MoonPaths, state: &mut MoonState) -> Result<()> {
    let docs = all_projection_docs(paths)?;
    let existing_paths = docs
        .iter()
        .map(|doc| doc.path.display().to_string())
        .collect::<std::collections::BTreeSet<_>>();
    let collection_names = state
        .embedded_projection_collections
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    for collection_name in collection_names {
        retain_embedded_projections_for_collection(state, &collection_name, |path, _| {
            existing_paths.contains(path)
        });
    }
    Ok(())
}

fn mark_global_embed_success(
    paths: &MoonPaths,
    state: &mut MoonState,
    embedded_at: u64,
) -> Result<usize> {
    let docs = all_projection_docs(paths)?;
    let mut recorded = 0usize;
    for doc in docs {
        let collection_name = infer_doc_collection(paths, &doc.path);
        let path_key = doc.path.display().to_string();
        let is_pending = embedded_projection_epoch(state, &collection_name, &path_key)
            .is_none_or(|last_embed| doc.mtime_epoch_secs > last_embed);
        if !is_pending {
            continue;
        }
        record_embedded_projection(
            state,
            &collection_name,
            path_key,
            embedded_at.max(doc.mtime_epoch_secs),
        );
        recorded += 1;
    }
    reconcile_embedded_projection_state(paths, state)?;
    Ok(recorded)
}

fn clear_drained_pending_collections(paths: &MoonPaths, state: &mut MoonState) -> Result<()> {
    let pending_collections = state
        .pending_embed_collections
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    for collection_name in pending_collections {
        let projection_root = projection_root_for_collection(paths, &collection_name);
        let docs = projection_docs(&projection_root)?;
        if pending_docs(state, &collection_name, &docs).is_empty() {
            state.pending_embed_collections.remove(&collection_name);
        }
    }
    Ok(())
}

pub fn run(
    paths: &MoonPaths,
    state: &mut MoonState,
    cfg: &MoonEmbedConfig,
    opts: &EmbedRunOptions,
) -> std::result::Result<EmbedRunSummary, EmbedRunError> {
    let started = Instant::now();
    let now_epoch = now_epoch_secs().map_err(|err| EmbedRunError::Failed(format!("{err:#}")))?;
    let projection_root = projection_root_for_collection(paths, &opts.collection_name);

    let docs = projection_docs(&projection_root)
        .map_err(|err| EmbedRunError::Failed(format!("{err:#}")))?;
    let pending = pending_docs(state, &opts.collection_name, &docs);
    let pending_before = pending.len();

    if opts.caller == EmbedCaller::Watcher {
        if !is_cooldown_ready(
            state.last_embed_trigger_epoch_secs,
            now_epoch,
            cfg.cooldown_secs,
        ) {
            return Ok(EmbedRunSummary {
                collection: opts.collection_name.clone(),
                mode: opts.caller.as_str().to_string(),
                capability: "missing".to_string(),
                requested_max_docs: opts.max_docs,
                selected_docs: 0,
                embedded_docs: 0,
                pending_before,
                pending_after: pending_before,
                elapsed_ms: started.elapsed().as_millis(),
                degraded: false,
                skip_reason: SkipReason::Cooldown.as_str().to_string(),
            });
        }

        if pending_before < cfg.min_pending_docs as usize {
            return Ok(EmbedRunSummary {
                collection: opts.collection_name.clone(),
                mode: opts.caller.as_str().to_string(),
                capability: "missing".to_string(),
                requested_max_docs: opts.max_docs,
                selected_docs: 0,
                embedded_docs: 0,
                pending_before,
                pending_after: pending_before,
                elapsed_ms: started.elapsed().as_millis(),
                degraded: false,
                skip_reason: SkipReason::None.as_str().to_string(),
            });
        }
    }

    let selected = pending
        .into_iter()
        .take(opts.max_docs.max(1))
        .collect::<Vec<_>>();
    let selected_docs = selected.len();
    if selected_docs == 0 {
        return Ok(EmbedRunSummary {
            collection: opts.collection_name.clone(),
            mode: opts.caller.as_str().to_string(),
            capability: "missing".to_string(),
            requested_max_docs: opts.max_docs,
            selected_docs: 0,
            embedded_docs: 0,
            pending_before,
            pending_after: pending_before,
            elapsed_ms: started.elapsed().as_millis(),
            degraded: false,
            skip_reason: SkipReason::None.as_str().to_string(),
        });
    }

    if opts.dry_run {
        return Ok(EmbedRunSummary {
            collection: opts.collection_name.clone(),
            mode: opts.caller.as_str().to_string(),
            capability: "missing".to_string(),
            requested_max_docs: opts.max_docs,
            selected_docs,
            embedded_docs: 0,
            pending_before,
            pending_after: pending_before,
            elapsed_ms: started.elapsed().as_millis(),
            degraded: false,
            skip_reason: SkipReason::None.as_str().to_string(),
        });
    }

    if opts.caller == EmbedCaller::Watcher {
        state.last_embed_trigger_epoch_secs = Some(now_epoch);
    }

    let probe = qmd::probe_embed_capability(&paths.qmd_bin);
    let mut skip_reason = SkipReason::None;

    match probe.capability {
        qmd::EmbedCapability::Bounded => {}
        qmd::EmbedCapability::UnboundedOnly => {}
        qmd::EmbedCapability::Missing => {
            if opts.caller == EmbedCaller::Watcher {
                return Ok(EmbedRunSummary {
                    collection: opts.collection_name.clone(),
                    mode: opts.caller.as_str().to_string(),
                    capability: probe.capability.as_str().to_string(),
                    requested_max_docs: opts.max_docs,
                    selected_docs,
                    embedded_docs: 0,
                    pending_before,
                    pending_after: pending_before,
                    elapsed_ms: started.elapsed().as_millis(),
                    degraded: true,
                    skip_reason: SkipReason::CapabilityMissing.as_str().to_string(),
                });
            }
            return Err(EmbedRunError::CapabilityMissing(probe.note));
        }
    }

    let _lock = match acquire_lock(paths, opts.caller, &opts.collection_name, now_epoch) {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            if opts.caller == EmbedCaller::Watcher {
                skip_reason = SkipReason::Locked;
                return Ok(EmbedRunSummary {
                    collection: opts.collection_name.clone(),
                    mode: opts.caller.as_str().to_string(),
                    capability: probe.capability.as_str().to_string(),
                    requested_max_docs: opts.max_docs,
                    selected_docs,
                    embedded_docs: 0,
                    pending_before,
                    pending_after: pending_before,
                    elapsed_ms: started.elapsed().as_millis(),
                    degraded: true,
                    skip_reason: skip_reason.as_str().to_string(),
                });
            }
            return Err(EmbedRunError::Locked(
                "another embed worker holds moon-embed.lock".to_string(),
            ));
        }
        Err(err) => {
            return Err(EmbedRunError::Failed(format!(
                "acquire-lock-failed error={err:#}"
            )));
        }
    };

    let (embedded_docs, exec) = match probe.capability {
        qmd::EmbedCapability::Bounded => {
            run_bounded_embed_with_backoff(paths, opts, selected_docs)?
        }
        qmd::EmbedCapability::UnboundedOnly => {
            let requested_batch = selected_docs.max(1);
            let exec =
                qmd::embed_global_batched(&paths.qmd_bin, requested_batch, opts.max_cycle_secs)
                    .map_err(|err| {
                        let timeout_text = opts
                            .max_cycle_secs
                            .map(|secs| secs.to_string())
                            .unwrap_or_else(|| "none".to_string());
                        EmbedRunError::Failed(format!(
                            "global-embed-failed max_docs_per_batch={requested_batch} timeout_secs={timeout_text} error={err:#}"
                        ))
                    })?;
            let recorded = mark_global_embed_success(paths, state, now_epoch)
                .map_err(|err| EmbedRunError::Failed(format!("{err:#}")))?;
            (recorded, exec)
        }
        qmd::EmbedCapability::Missing => unreachable!("missing capability handled above"),
    };

    if qmd::output_indicates_embed_status_failed(&exec.stdout, &exec.stderr) {
        return Err(EmbedRunError::StatusFailed(
            "qmd output indicates failed status".to_string(),
        ));
    }

    if probe.capability == qmd::EmbedCapability::Bounded {
        for doc in selected.iter().take(embedded_docs) {
            record_embedded_projection(
                state,
                &opts.collection_name,
                doc.path.display().to_string(),
                now_epoch.max(doc.mtime_epoch_secs),
            );
        }
        reconcile_embedded_projection_state(paths, state)
            .map_err(|err| EmbedRunError::Failed(format!("{err:#}")))?;
    }

    let pending_after = pending_docs(state, &opts.collection_name, &docs).len();
    clear_drained_pending_collections(paths, state)
        .map_err(|err| EmbedRunError::Failed(format!("{err:#}")))?;

    Ok(EmbedRunSummary {
        collection: opts.collection_name.clone(),
        mode: opts.caller.as_str().to_string(),
        capability: probe.capability.as_str().to_string(),
        requested_max_docs: opts.max_docs,
        selected_docs,
        embedded_docs,
        pending_before,
        pending_after,
        elapsed_ms: started.elapsed().as_millis(),
        degraded: false,
        skip_reason: skip_reason.as_str().to_string(),
    })
}

pub fn run_manual_now(
    paths: &MoonPaths,
    state: &mut MoonState,
    cfg: &MoonEmbedConfig,
    collection_name: &str,
) -> std::result::Result<EmbedRunSummary, EmbedRunError> {
    run(
        paths,
        state,
        cfg,
        &EmbedRunOptions {
            collection_name: collection_name.to_string(),
            max_docs: cfg.max_docs_per_cycle as usize,
            dry_run: false,
            caller: EmbedCaller::Manual,
            max_cycle_secs: Some(cfg.max_cycle_secs),
        },
    )
}

pub fn clear_pending_collection_if_drained(
    paths: &MoonPaths,
    state: &mut MoonState,
    collection_name: &str,
    summary: &EmbedRunSummary,
) {
    if summary.pending_after == 0 || (summary.pending_before == 0 && summary.selected_docs == 0) {
        state.pending_embed_collections.remove(collection_name);
    }
    let _ = clear_drained_pending_collections(paths, state);
}

#[cfg(test)]
mod tests {
    use super::{ProjectionDoc, pending_docs};
    use crate::moon::state::MoonState;
    use std::path::PathBuf;

    #[test]
    fn pending_docs_detects_missing_and_stale_epochs() {
        let mut state = MoonState::default();
        state
            .embedded_projection_collections
            .entry("history_hot".to_string())
            .or_default()
            .insert("/tmp/a.md".to_string(), 100);
        state
            .embedded_projection_collections
            .entry("history_hot".to_string())
            .or_default()
            .insert("/tmp/b.md".to_string(), 300);

        let docs = vec![
            ProjectionDoc {
                path: PathBuf::from("/tmp/a.md"),
                mtime_epoch_secs: 200,
            },
            ProjectionDoc {
                path: PathBuf::from("/tmp/b.md"),
                mtime_epoch_secs: 200,
            },
            ProjectionDoc {
                path: PathBuf::from("/tmp/c.md"),
                mtime_epoch_secs: 1,
            },
        ];

        let pending = pending_docs(&state, "history_hot", &docs);
        let names = pending
            .iter()
            .map(|doc| doc.path.display().to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec!["/tmp/a.md".to_string(), "/tmp/c.md".to_string()]
        );
    }
}
