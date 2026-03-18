use crate::moon::config::{
    MoonHotCollectionLifecycleCommandMode, MoonHotCollectionLifecycleMode, load_config,
};
use crate::moon::daemon_lock::{DaemonLockPayload, daemon_lock_path};
use crate::moon::distill::{
    DistillInput, DistillOutput, WisdomDistillInput, run_distillation, run_wisdom_distillation,
};
use crate::moon::embed::{self, EmbedCaller, EmbedRunOptions};
use crate::moon::files::{file_epoch_secs, gather_files_with_extension};
use crate::moon::paths::{MoonPaths, resolve_paths};
use crate::moon::project::{self, ProjectLane, ProjectRunOptions};
use crate::moon::qmd;
use crate::moon::state::{RawSessionCursor, is_hot_embed_collection, load, save, state_file_path};
use anyhow::{Context, Result};
use chrono::{LocalResult, TimeZone, Timelike};
use chrono_tz::Tz;
use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

const BUILD_UUID: &str = env!("BUILD_UUID");
const HOT_COLLECTION_LIFECYCLE_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Clone, Copy, Default)]
pub struct WatchRunOptions {
    pub force_distill_now: bool,
    pub dry_run: bool,
}

#[derive(Debug, Clone)]
pub struct WatchCycleOutcome {
    pub state_file: String,
    pub heartbeat_epoch_secs: u64,
    pub poll_interval_secs: u64,
    pub distill_max_per_cycle: u64,
    pub pending_raw_sessions: usize,
    pub projected_sessions: usize,
    pub pending_mlib_docs: usize,
    pub distill_runs: usize,
    pub pending_embed_collections: usize,
    pub embed_runs: usize,
    pub embed_last_summary: Option<String>,
    pub hot_collection_lifecycle_mode: String,
    pub hot_collection_lifecycle_command_mode: String,
    pub syns_due: bool,
    pub distill: Option<DistillOutput>,
    pub syns_result: Option<String>,
}

#[derive(Debug, Clone)]
struct ProjectionDoc {
    path: PathBuf,
    mtime_epoch_secs: u64,
}

#[derive(Debug, Clone)]
struct RawSessionDoc {
    session_id: String,
    source_path: PathBuf,
    mtime_epoch_secs: u64,
    bytes: u64,
    lines: u64,
}

fn hot_collection_lifecycle_summary(
    mode: MoonHotCollectionLifecycleMode,
    command_mode: MoonHotCollectionLifecycleCommandMode,
    probe: &qmd::CollectionLifecycleCapabilityProbe,
) -> Option<String> {
    match mode {
        MoonHotCollectionLifecycleMode::Disabled => Some(format!(
            "hot_lifecycle=disabled capability={} note={}",
            probe.capability.as_str(),
            probe.note
        )),
        MoonHotCollectionLifecycleMode::Degrade
            if probe.capability == qmd::CollectionLifecycleCapability::Missing =>
        {
            Some(format!(
                "hot_lifecycle=degraded reason=lifecycle-capability-missing command_mode={} capability={} note={}",
                command_mode.as_str(),
                probe.capability.as_str(),
                probe.note
            ))
        }
        _ => None,
    }
}

fn now_epoch_secs() -> Result<u64> {
    if let Ok(raw) = std::env::var("MOON_WATCH_FAKE_NOW_EPOCH_SECS") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return trimmed
                .parse::<u64>()
                .context("invalid MOON_WATCH_FAKE_NOW_EPOCH_SECS");
        }
    }
    crate::moon::util::now_epoch_secs()
}

fn session_byte_line_stats(path: &Path) -> Result<(u64, u64)> {
    let bytes = fs::metadata(path)
        .with_context(|| format!("failed to stat {}", path.display()))?
        .len();
    let content = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    let line_breaks = content.iter().filter(|byte| **byte == b'\n').count() as u64;
    let lines = if content.is_empty() {
        0
    } else if content.last() == Some(&b'\n') {
        line_breaks
    } else {
        line_breaks + 1
    };
    Ok((bytes, lines))
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

fn list_mlib_docs(paths: &MoonPaths) -> Result<Vec<ProjectionDoc>> {
    let mut docs = Vec::new();
    gather_projection_docs(&paths.mlib_dir, &mut docs)?;
    docs.sort_by(|a, b| {
        a.mtime_epoch_secs
            .cmp(&b.mtime_epoch_secs)
            .then_with(|| a.path.cmp(&b.path))
    });
    Ok(docs)
}

fn pending_mlib_docs(
    paths: &MoonPaths,
    state: &crate::moon::state::MoonState,
) -> Result<Vec<ProjectionDoc>> {
    Ok(list_mlib_docs(paths)?
        .into_iter()
        .filter(|doc| {
            let key = doc.path.display().to_string();
            match state.distilled_archives.get(&key) {
                None => true,
                Some(last_distill) => doc.mtime_epoch_secs > *last_distill,
            }
        })
        .collect())
}

fn gather_raw_sessions(root: &Path, out: &mut Vec<RawSessionDoc>) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }

    for entry in
        fs::read_dir(root).with_context(|| format!("failed to read raw dir {}", root.display()))?
    {
        let entry = entry.with_context(|| format!("failed to read entry in {}", root.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to stat {}", path.display()))?;
        if file_type.is_dir() {
            gather_raw_sessions(&path, out)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        if path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_none_or(|ext| !ext.eq_ignore_ascii_case("jsonl"))
        {
            continue;
        }

        let session_id = path
            .file_stem()
            .and_then(|value| value.to_str())
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .unwrap_or("session")
            .to_string();
        let (bytes, lines) = session_byte_line_stats(&path)?;

        out.push(RawSessionDoc {
            session_id,
            source_path: path.clone(),
            mtime_epoch_secs: file_epoch_secs(&path),
            bytes,
            lines,
        });
    }

    Ok(())
}

fn list_raw_sessions(paths: &MoonPaths) -> Result<Vec<RawSessionDoc>> {
    let mut docs = Vec::new();
    gather_raw_sessions(&paths.raw_dir, &mut docs)?;

    // Keep the latest doc per session id if duplicates exist.
    let mut by_session = BTreeMap::<String, RawSessionDoc>::new();
    for doc in docs {
        match by_session.get(&doc.session_id) {
            None => {
                by_session.insert(doc.session_id.clone(), doc);
            }
            Some(existing)
                if doc.mtime_epoch_secs > existing.mtime_epoch_secs
                    || (doc.mtime_epoch_secs == existing.mtime_epoch_secs
                        && doc.source_path > existing.source_path) =>
            {
                by_session.insert(doc.session_id.clone(), doc);
            }
            Some(_) => {}
        }
    }

    let mut out = by_session.into_values().collect::<Vec<_>>();
    out.sort_by(|a, b| {
        a.mtime_epoch_secs
            .cmp(&b.mtime_epoch_secs)
            .then_with(|| a.session_id.cmp(&b.session_id))
    });
    Ok(out)
}

fn pending_raw_sessions(
    paths: &MoonPaths,
    state: &crate::moon::state::MoonState,
) -> Result<Vec<RawSessionDoc>> {
    Ok(list_raw_sessions(paths)?
        .into_iter()
        .filter(|doc| match state.raw_session_cursors.get(&doc.session_id) {
            None => true,
            Some(cursor) => cursor.bytes != doc.bytes || cursor.lines != doc.lines,
        })
        .collect())
}

fn pending_embed_collections(state: &crate::moon::state::MoonState) -> Vec<String> {
    let mut queued = state
        .pending_embed_collections
        .iter()
        .map(|(collection, epoch)| (*epoch, collection.clone()))
        .collect::<Vec<_>>();
    queued.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    queued
        .into_iter()
        .map(|(_, collection)| collection)
        .collect()
}

fn residential_tz_name(cfg: &crate::moon::config::MoonConfig) -> String {
    let name = cfg.distill.residential_timezone.trim();
    if name.is_empty() {
        "UTC".to_string()
    } else {
        name.to_string()
    }
}

fn parse_residential_tz(cfg: &crate::moon::config::MoonConfig) -> Tz {
    residential_tz_name(cfg)
        .parse::<Tz>()
        .unwrap_or(chrono_tz::UTC)
}

fn day_key_for_epoch_in_timezone(epoch_secs: u64, tz: Tz) -> String {
    let dt = match tz.timestamp_opt(epoch_secs as i64, 0) {
        LocalResult::Single(v) => v,
        _ => tz.from_utc_datetime(&chrono::Utc::now().naive_utc()),
    };
    dt.format("%Y-%m-%d").to_string()
}

fn previous_day_key_for_epoch_in_timezone(epoch_secs: u64, tz: Tz) -> String {
    let dt = match tz.timestamp_opt(epoch_secs as i64, 0) {
        LocalResult::Single(v) => v,
        _ => tz.from_utc_datetime(&chrono::Utc::now().naive_utc()),
    };
    let previous_day = dt.date_naive() - chrono::Duration::days(1);
    previous_day.format("%Y-%m-%d").to_string()
}

fn syns_due_now(state: &crate::moon::state::MoonState, now_epoch_secs: u64, tz: Tz) -> bool {
    let now_local = match tz.timestamp_opt(now_epoch_secs as i64, 0) {
        LocalResult::Single(v) => v,
        _ => return false,
    };
    if now_local.hour() != 0 {
        return false;
    }
    let today_key = now_local.format("%Y-%m-%d").to_string();
    let last_key = state
        .last_syns_trigger_epoch_secs
        .map(|epoch| day_key_for_epoch_in_timezone(epoch, tz));
    last_key.as_deref() != Some(today_key.as_str())
}

fn lock_payload(paths: &MoonPaths, now_epoch_secs: u64) -> DaemonLockPayload {
    DaemonLockPayload {
        pid: std::process::id(),
        started_at_epoch_secs: now_epoch_secs,
        build_uuid: BUILD_UUID.to_string(),
        moon_home: paths.moon_home.display().to_string(),
    }
}

fn write_daemon_lock(paths: &MoonPaths, now_epoch_secs: u64) -> Result<PathBuf> {
    fs::create_dir_all(&paths.logs_dir)
        .with_context(|| format!("failed to create {}", paths.logs_dir.display()))?;
    let lock_path = daemon_lock_path(paths);
    let payload = lock_payload(paths, now_epoch_secs);
    fs::write(
        &lock_path,
        format!("{}\n", serde_json::to_string(&payload)?),
    )
    .with_context(|| format!("failed to write {}", lock_path.display()))?;
    Ok(lock_path)
}

fn remove_daemon_lock(paths: &MoonPaths) {
    let lock_path = daemon_lock_path(paths);
    match fs::remove_file(&lock_path) {
        Ok(_) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(_) => {}
    }
}

pub fn run_once() -> Result<WatchCycleOutcome> {
    run_once_with_options(WatchRunOptions::default())
}

pub fn run_once_with_options(run_opts: WatchRunOptions) -> Result<WatchCycleOutcome> {
    let paths = resolve_paths()?;
    qmd::install_runtime_env(&paths);
    let cfg = load_config()?;
    let mut state = load(&paths)?;
    let now_epoch = now_epoch_secs()?;
    let tz = parse_residential_tz(&cfg);

    let pending_raw = pending_raw_sessions(&paths, &state)?;
    let pending_raw_count = pending_raw.len();
    let mut projected_sessions = 0usize;
    if !run_opts.dry_run {
        for raw in pending_raw
            .into_iter()
            .take(cfg.distill.max_per_cycle as usize)
        {
            let source_path = raw.source_path.display().to_string();
            project::run_and_mark_embed_pending(
                &paths,
                &mut state,
                &ProjectRunOptions {
                    source_path: Some(source_path),
                    session_id: Some(raw.session_id.clone()),
                    lane: ProjectLane::Library,
                    dry_run: false,
                },
            )
            .with_context(|| {
                format!(
                    "watcher project failed for session `{}` from {}",
                    raw.session_id,
                    raw.source_path.display()
                )
            })?;
            state.raw_session_cursors.insert(
                raw.session_id,
                RawSessionCursor {
                    bytes: raw.bytes,
                    lines: raw.lines,
                },
            );
            projected_sessions += 1;
        }
    }

    let pending_embed_count = state.pending_embed_collections.len();
    let mut embed_runs = 0usize;
    let mut embed_last_summary = None;
    let hot_lifecycle_mode = cfg.hot_collection.lifecycle_mode;
    let hot_lifecycle_command_mode = cfg.hot_collection.lifecycle_command_mode;
    if !run_opts.dry_run {
        for collection_name in pending_embed_collections(&state) {
            let lifecycle_summary = if is_hot_embed_collection(&collection_name) {
                match ensure_hot_collection_lifecycle(
                    &paths,
                    &collection_name,
                    hot_lifecycle_mode,
                    hot_lifecycle_command_mode,
                ) {
                    Ok(detail) => {
                        if detail.starts_with("hot_lifecycle=ok") {
                            state
                                .managed_hot_collections
                                .insert(collection_name.clone(), now_epoch);
                        }
                        detail
                    }
                    Err(err) if hot_lifecycle_mode == MoonHotCollectionLifecycleMode::Strict => {
                        return Err(anyhow::anyhow!(
                            "watcher strict mode hot collection lifecycle failed for `{}`: {err:#}",
                            collection_name
                        ));
                    }
                    Err(err) => format!(
                        "hot_lifecycle=degraded error={}",
                        crate::moon::util::truncate_with_ellipsis(&format!("{err:#}"), 200)
                    ),
                }
            } else {
                "hot_lifecycle=not-applicable".to_string()
            };

            let summary = embed::run(
                &paths,
                &mut state,
                &cfg.embed,
                &EmbedRunOptions {
                    collection_name: collection_name.clone(),
                    max_docs: cfg.embed.max_docs_per_cycle as usize,
                    dry_run: false,
                    caller: EmbedCaller::Watcher,
                    max_cycle_secs: Some(cfg.embed.max_cycle_secs),
                },
            )
            .map_err(|err| anyhow::anyhow!("watcher embed failed: {err}"))?;
            if hot_lifecycle_mode == MoonHotCollectionLifecycleMode::Strict
                && is_hot_embed_collection(&collection_name)
                && summary.degraded
            {
                return Err(anyhow::anyhow!(
                    "watcher strict mode rejects degraded embed result for `{}`: skip_reason={} capability={} pending_before={} pending_after={}",
                    summary.collection,
                    summary.skip_reason,
                    summary.capability,
                    summary.pending_before,
                    summary.pending_after
                ));
            }
            embed_runs += 1;
            embed_last_summary = Some(format!(
                "collection={} embedded_docs={} pending_before={} pending_after={} skip_reason={} degraded={} {}",
                summary.collection,
                summary.embedded_docs,
                summary.pending_before,
                summary.pending_after,
                summary.skip_reason,
                summary.degraded,
                lifecycle_summary
            ));

            if summary.pending_after == 0
                || (summary.pending_before == 0 && summary.selected_docs == 0)
            {
                state.pending_embed_collections.remove(&collection_name);
            }
        }
    }

    let pending_docs = pending_mlib_docs(&paths, &state)?;
    let pending_count = pending_docs.len();
    let mut last_distill = None;
    let mut distill_runs = 0usize;

    if !run_opts.dry_run {
        for doc in pending_docs
            .into_iter()
            .take(cfg.distill.max_per_cycle as usize)
        {
            let session_id = doc
                .path
                .file_stem()
                .and_then(|v| v.to_str())
                .unwrap_or("session")
                .to_string();
            let out = run_distillation(
                &paths,
                &DistillInput {
                    session_id,
                    archive_path: doc.path.display().to_string(),
                    archive_text: String::new(),
                    archive_epoch_secs: Some(doc.mtime_epoch_secs),
                },
            )?;
            state
                .distilled_archives
                .insert(doc.path.display().to_string(), now_epoch);
            state.last_distill_trigger_epoch_secs = Some(now_epoch);
            distill_runs += 1;
            last_distill = Some(out);
            if !run_opts.force_distill_now && distill_runs >= cfg.distill.max_per_cycle as usize {
                break;
            }
        }
    }

    let syns_due = syns_due_now(&state, now_epoch, tz);
    let mut syns_result = None;
    if syns_due {
        let yesterday_key = previous_day_key_for_epoch_in_timezone(now_epoch, tz);
        let yesterday_source = paths.memory_dir.join(format!("{yesterday_key}.md"));
        let sources = if yesterday_source.exists() {
            vec![
                yesterday_source.display().to_string(),
                paths.memory_file.display().to_string(),
            ]
        } else {
            Vec::new()
        };

        if run_opts.dry_run {
            syns_result = Some(format!(
                "dry-run trigger=watch-midnight sources={}",
                sources.join(",")
            ));
        } else {
            let out = run_wisdom_distillation(
                &paths,
                &WisdomDistillInput {
                    trigger: "watch-midnight".to_string(),
                    day_epoch_secs: Some(now_epoch),
                    source_paths: sources,
                    dry_run: false,
                },
            )?;
            state.last_syns_trigger_epoch_secs = Some(now_epoch);
            syns_result = Some(format!(
                "provider={} summary_path={}",
                out.provider, out.summary_path
            ));
        }
    }

    state.last_heartbeat_epoch_secs = now_epoch;
    let state_file = if run_opts.dry_run {
        state_file_path(&paths)
    } else {
        save(&paths, &state)?
    };

    Ok(WatchCycleOutcome {
        state_file: state_file.display().to_string(),
        heartbeat_epoch_secs: now_epoch,
        poll_interval_secs: cfg.watcher.poll_interval_secs,
        distill_max_per_cycle: cfg.distill.max_per_cycle,
        pending_raw_sessions: pending_raw_count,
        projected_sessions,
        pending_mlib_docs: pending_count,
        distill_runs,
        pending_embed_collections: pending_embed_count,
        embed_runs,
        embed_last_summary,
        hot_collection_lifecycle_mode: hot_lifecycle_mode.as_str().to_string(),
        hot_collection_lifecycle_command_mode: hot_lifecycle_command_mode.as_str().to_string(),
        syns_due,
        distill: last_distill,
        syns_result,
    })
}

fn ensure_hot_collection_lifecycle(
    paths: &MoonPaths,
    collection_name: &str,
    lifecycle_mode: MoonHotCollectionLifecycleMode,
    command_mode: MoonHotCollectionLifecycleCommandMode,
) -> Result<String> {
    let probe = qmd::probe_collection_lifecycle_capability(&paths.qmd_bin, command_mode);
    if let Some(summary) = hot_collection_lifecycle_summary(lifecycle_mode, command_mode, &probe) {
        return Ok(summary);
    }

    let collection_dir =
        crate::moon::state::hot_projection_dir_for_collection(paths, collection_name);
    let create = qmd::collection_create(
        &paths.qmd_bin,
        collection_name,
        &collection_dir,
        command_mode,
        Some(HOT_COLLECTION_LIFECYCLE_TIMEOUT_SECS),
    )?;
    Ok(format!(
        "hot_lifecycle=ok capability={} note={} register_cmd=`{}` register_fallback={}",
        probe.capability.as_str(),
        probe.note,
        create.command,
        create.used_fallback,
    ))
}

pub fn run_daemon() -> Result<()> {
    let paths = resolve_paths()?;
    let cfg = load_config()?;
    let now_epoch = now_epoch_secs()?;
    let _lock_path = write_daemon_lock(&paths, now_epoch)?;

    let keep_running = Arc::new(AtomicBool::new(true));
    {
        let keep_running = Arc::clone(&keep_running);
        ctrlc::set_handler(move || {
            keep_running.store(false, Ordering::SeqCst);
        })
        .context("failed to install ctrl-c handler")?;
    }

    while keep_running.load(Ordering::SeqCst) {
        let _ = run_once();
        for _ in 0..cfg.watcher.poll_interval_secs {
            if !keep_running.load(Ordering::SeqCst) {
                break;
            }
            thread::sleep(Duration::from_secs(1));
        }
    }

    remove_daemon_lock(&paths);
    Ok(())
}
