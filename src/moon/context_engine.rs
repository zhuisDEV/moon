use anyhow::{Context, Result};
use std::fs;

use crate::moon::assemble::{
    AssembleOutput, assemble_context, embedding_index_anchor_from_state,
    resolve_input as resolve_assemble_input, write_assembly_output,
};
use crate::moon::audit;
use crate::moon::cleanse::{CleanseInput, render_summary_document, run_cleanse};
use crate::moon::config::{
    MoonContextConfig, MoonHotCollectionLifecycleCommandMode, MoonHotCollectionLifecycleMode,
    load_config,
};
use crate::moon::context_packet::{
    ContextPacketInput, ContextPacketOutput, build_context_packet, write_context_packet_output,
};
use crate::moon::distill::load_source_excerpt;
use crate::moon::embed;
use crate::moon::paths::MoonPaths;
use crate::moon::project::{self, ProjectLane, ProjectRunOptions};
use crate::moon::qmd;
use crate::moon::record;
use crate::moon::state::{
    MoonState, hot_embed_collection_for_session, hot_projection_dir_for_collection, load,
    remove_embedded_projection_collection, save,
};
use crate::moon::util::now_epoch_secs;

const DEFAULT_CONTEXT_WINDOW_TOKENS: u64 = 200_000;
const HOT_COLLECTION_LIFECYCLE_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Clone, Copy)]
pub struct PressureSnapshot {
    pub used_tokens: u64,
    pub max_tokens: u64,
}

#[derive(Debug, Clone, Default)]
pub struct CheckpointOptions {
    pub source_path: Option<String>,
    pub session_id: Option<String>,
    pub pressure: Option<PressureSnapshot>,
    pub force_cleanse: bool,
    pub replay_has_compaction_summary: bool,
}

#[derive(Debug, Clone)]
pub struct CheckpointOutput {
    pub session_id: String,
    pub record_target_path: String,
    pub cleanse_summary_path: Option<String>,
    pub embed_now: Option<String>,
    pub cleanse_reason: String,
    pub assembly_output_path: String,
    pub assembly: AssembleOutput,
    pub context_packet_output_path: Option<String>,
    pub context_packet: Option<ContextPacketOutput>,
}

#[derive(Debug, Clone)]
struct HotCollectionLifecycleRun {
    summary: String,
    degraded: bool,
}

fn hot_collection_lifecycle_probe_summary(
    mode: MoonHotCollectionLifecycleMode,
    probe: &qmd::CollectionLifecycleCapabilityProbe,
    command_mode: MoonHotCollectionLifecycleCommandMode,
) -> Option<HotCollectionLifecycleRun> {
    match mode {
        MoonHotCollectionLifecycleMode::Disabled => Some(HotCollectionLifecycleRun {
            summary: format!(
                "status=disabled reason=config-disabled capability={} note={}",
                probe.capability.as_str(),
                probe.note
            ),
            degraded: false,
        }),
        MoonHotCollectionLifecycleMode::Degrade
            if probe.capability == qmd::CollectionLifecycleCapability::Missing =>
        {
            Some(HotCollectionLifecycleRun {
                summary: format!(
                    "status=degraded reason=lifecycle-capability-missing command_mode={} capability={} note={}",
                    command_mode.as_str(),
                    probe.capability.as_str(),
                    probe.note
                ),
                degraded: true,
            })
        }
        _ => None,
    }
}

#[derive(Debug, Clone, Copy)]
struct CleanseThresholds {
    window_tokens: u64,
    trigger_tokens: u64,
    emergency_tokens: u64,
}

pub fn run_checkpoint(paths: &MoonPaths, opts: &CheckpointOptions) -> Result<CheckpointOutput> {
    qmd::install_runtime_env(paths);
    let cfg = load_config().unwrap_or_default();
    let plan = record::plan_record(
        paths,
        opts.source_path.as_deref(),
        opts.session_id.as_deref(),
    )?;
    let _record_result = record::execute_record(paths, &plan)?;
    let (should_cleanse, cleanse_reason, usage_ratio) =
        evaluate_cleanse_need(opts.pressure, opts.force_cleanse);

    let mut state = load(paths)?;
    let previous_session_id = state.last_session_id.clone();
    state.last_session_id = Some(plan.session_id.clone());
    state.last_usage_ratio = usage_ratio;
    let (hot_lifecycle_mode, hot_lifecycle_command_mode, hot_lifecycle_mode_note) =
        resolve_hot_collection_lifecycle_policy();
    let hot_lifecycle = run_hot_collection_lifecycle(
        paths,
        &mut state,
        hot_lifecycle_mode,
        &plan.session_id,
        previous_session_id.as_deref(),
        hot_lifecycle_command_mode,
    );
    if hot_lifecycle_mode == MoonHotCollectionLifecycleMode::Strict && hot_lifecycle.degraded {
        anyhow::bail!(
            "hot collection lifecycle failed in strict mode: {}",
            hot_lifecycle.summary
        );
    }
    let hot_lifecycle_summary = format!(
        "mode={} command_mode={}{} {}",
        hot_lifecycle_mode.as_str(),
        hot_lifecycle_command_mode.as_str(),
        hot_lifecycle_mode_note
            .as_deref()
            .map(|note| format!(" note={note}"))
            .unwrap_or_default(),
        hot_lifecycle.summary
    );
    let mut hot_prune_summary = "not-triggered".to_string();
    if previous_session_id
        .as_deref()
        .is_some_and(|previous| previous != plan.session_id)
    {
        let pruned = project::prune_hot_cache_for_session(paths, &mut state, &plan.session_id)?;
        hot_prune_summary = format!(
            "removed_docs={} removed_index_entries={} removed_distill_entries={} removed_pending_hot_collections={}",
            pruned.removed_docs,
            pruned.removed_index_entries,
            pruned.removed_distill_entries,
            pruned.removed_pending_hot_collections
        );
    }
    let mut embed_now = "not-triggered".to_string();
    let projected_path = Some(run_project_checkpoint(
        paths,
        &mut state,
        &plan.session_id,
        &plan.target_path,
    )?);
    let cleanse_summary_path = if should_cleanse {
        let queued_epoch_secs = now_epoch_secs()?;
        state.last_compaction_trigger_epoch_secs = Some(queued_epoch_secs);
        embed_now = run_embed_now(paths, &mut state, &plan.session_id);
        Some(run_cleanse_checkpoint(
            paths,
            &plan.session_id,
            &plan.target_path,
        )?)
    } else {
        None
    };

    let raw_source_path = plan.target_path.display().to_string();
    let mut assemble_input = resolve_assemble_input(
        paths,
        Some(raw_source_path.as_str()),
        Some(&plan.session_id),
    )?;
    assemble_input.embedding_index_anchor = Some(embedding_index_anchor_from_state(
        paths,
        &state,
        &plan.session_id,
    ));
    let assembly = assemble_context(&assemble_input)?;
    let assembly_output_path = write_assembly_output(paths, &plan.session_id, &assembly.content)?;
    let (context_packet, context_packet_output_path) = if cfg.context_packet.enabled {
        let packet = build_context_packet(
            paths,
            &state,
            &cfg.context_packet,
            &ContextPacketInput {
                session_id: plan.session_id.clone(),
                raw_source_path: plan.target_path.clone(),
                cleanse_summary_path: cleanse_summary_path.as_ref().map(std::path::PathBuf::from),
                replay_has_compaction_summary: opts.replay_has_compaction_summary,
            },
        )?;
        let packet_output_path =
            write_context_packet_output(paths, &plan.session_id, &packet.content)?;
        state.last_context_packet_session_id = Some(plan.session_id.clone());
        state.last_context_packet_epoch_secs = Some(packet.packet_at_epoch_secs);
        state.last_context_packet_generation = Some(packet.generation.clone());
        state.last_context_packet_candidate_count = Some(packet.candidate_count);
        (Some(packet), Some(packet_output_path.display().to_string()))
    } else {
        state.last_context_packet_session_id = None;
        state.last_context_packet_epoch_secs = None;
        state.last_context_packet_generation = None;
        state.last_context_packet_candidate_count = None;
        (None, None)
    };

    state.last_assembly_session_id = Some(plan.session_id.clone());
    state.last_assembly_epoch_secs = Some(assembly.assembled_at_epoch_secs);
    let _ = save(paths, &state)?;

    let _ = audit::append_event(
        paths,
        "context-engine",
        "ok",
        &format!(
            "session_id={} record={} project={} cleanse={} embed_now={} assembly={} packet={} reason={} hot_lifecycle={} hot_prune={}",
            plan.session_id,
            plan.target_path.display(),
            projected_path.as_deref().unwrap_or("none"),
            cleanse_summary_path.as_deref().unwrap_or("none"),
            embed_now,
            assembly_output_path.display(),
            context_packet_output_path.as_deref().unwrap_or("none"),
            cleanse_reason,
            hot_lifecycle_summary,
            hot_prune_summary
        ),
    );

    Ok(CheckpointOutput {
        session_id: plan.session_id,
        record_target_path: plan.target_path.display().to_string(),
        cleanse_summary_path,
        embed_now: should_cleanse.then_some(embed_now),
        cleanse_reason,
        assembly_output_path: assembly_output_path.display().to_string(),
        assembly,
        context_packet_output_path,
        context_packet,
    })
}

fn evaluate_cleanse_need(
    pressure: Option<PressureSnapshot>,
    force_cleanse: bool,
) -> (bool, String, Option<f64>) {
    if force_cleanse {
        return (true, "forced".to_string(), pressure_ratio(pressure));
    }

    let Some(pressure) = pressure else {
        return (false, "no-pressure-snapshot".to_string(), None);
    };
    let ratio = pressure_ratio(Some(pressure));
    let thresholds = resolve_cleanse_thresholds(Some(pressure));
    if pressure.used_tokens >= thresholds.emergency_tokens {
        return (
            true,
            format!(
                "emergency-used-tokens>={} window_tokens={}",
                thresholds.emergency_tokens, thresholds.window_tokens
            ),
            ratio,
        );
    }
    if pressure.used_tokens >= thresholds.trigger_tokens {
        return (
            true,
            format!(
                "trigger-used-tokens>={} window_tokens={}",
                thresholds.trigger_tokens, thresholds.window_tokens
            ),
            ratio,
        );
    }
    (
        false,
        format!(
            "below-trigger used_tokens={} trigger_tokens={} window_tokens={}",
            pressure.used_tokens, thresholds.trigger_tokens, thresholds.window_tokens
        ),
        ratio,
    )
}

fn pressure_ratio(pressure: Option<PressureSnapshot>) -> Option<f64> {
    let pressure = pressure?;
    if pressure.max_tokens == 0 {
        return None;
    }
    Some(pressure.used_tokens as f64 / pressure.max_tokens as f64)
}

fn cleanse_thresholds_from_context(
    context: Option<&MoonContextConfig>,
    pressure: Option<PressureSnapshot>,
) -> CleanseThresholds {
    let default_context = MoonContextConfig::default();
    let window_tokens = context
        .and_then(|context| context.window_tokens)
        .filter(|tokens| *tokens > 0)
        .or_else(|| {
            pressure
                .map(|pressure| pressure.max_tokens)
                .filter(|tokens| *tokens > 0)
        })
        .unwrap_or(DEFAULT_CONTEXT_WINDOW_TOKENS);
    let trigger_ratio = context
        .map(|context| context.cleanse_trigger_ratio)
        .unwrap_or(default_context.cleanse_trigger_ratio);
    let emergency_ratio = context
        .map(|context| context.cleanse_emergency_ratio)
        .unwrap_or(default_context.cleanse_emergency_ratio);

    let ratio_tokens = |ratio: f64| -> u64 {
        ((window_tokens as f64) * ratio)
            .ceil()
            .max(1.0)
            .min(u64::MAX as f64) as u64
    };

    CleanseThresholds {
        window_tokens,
        trigger_tokens: ratio_tokens(trigger_ratio),
        emergency_tokens: ratio_tokens(emergency_ratio),
    }
}

fn resolve_cleanse_thresholds(pressure: Option<PressureSnapshot>) -> CleanseThresholds {
    match load_config() {
        Ok(cfg) => cleanse_thresholds_from_context(cfg.context.as_ref(), pressure),
        Err(_) => cleanse_thresholds_from_context(None, pressure),
    }
}

fn run_cleanse_checkpoint(
    paths: &MoonPaths,
    session_id: &str,
    raw_target_path: &std::path::Path,
) -> Result<String> {
    let raw_target_path_str = raw_target_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("record target path is not valid UTF-8"))?
        .to_string();
    let source_excerpt = load_source_excerpt(&raw_target_path_str).with_context(|| {
        format!(
            "failed to derive cleanse input from {}",
            raw_target_path.display()
        )
    })?;
    let output = run_cleanse(&CleanseInput {
        session_id: session_id.to_string(),
        source_path: raw_target_path_str.clone(),
        source_excerpt,
    })?;

    fs::create_dir_all(&paths.cleanse_dir)
        .with_context(|| format!("failed to create {}", paths.cleanse_dir.display()))?;
    let summary_path = paths.cleanse_dir.join(format!("{session_id}.md"));
    let rendered = render_summary_document(
        session_id,
        &raw_target_path_str,
        &output.provider,
        &output.model,
        output.created_at_epoch_secs,
        &output.summary,
    );
    fs::write(&summary_path, rendered.as_bytes())
        .with_context(|| format!("failed to write {}", summary_path.display()))?;

    Ok(summary_path.display().to_string())
}

fn run_project_checkpoint(
    paths: &MoonPaths,
    state: &mut MoonState,
    session_id: &str,
    raw_target_path: &std::path::Path,
) -> Result<String> {
    let source_path = raw_target_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("record target path is not valid UTF-8"))?
        .to_string();
    let output = project::run_and_mark_embed_pending(
        paths,
        state,
        &ProjectRunOptions {
            source_path: Some(source_path),
            session_id: Some(session_id.to_string()),
            lane: ProjectLane::Hot,
            dry_run: false,
        },
    )
    .with_context(|| format!("project checkpoint failed for session `{session_id}`"))?;
    Ok(output.target_path.display().to_string())
}

fn run_embed_now(paths: &MoonPaths, state: &mut MoonState, session_id: &str) -> String {
    let cfg = match load_config() {
        Ok(cfg) => cfg,
        Err(err) => return format!("degraded config-load-failed={err:#}"),
    };
    let collection_name = hot_embed_collection_for_session(session_id);

    match embed::run_manual_now(paths, state, &cfg.embed, &collection_name) {
        Ok(summary) => {
            embed::clear_pending_collection_if_drained(paths, state, &collection_name, &summary);
            format!(
                "ok collection={} selected={} embedded={} pending_before={} pending_after={} skip_reason={} degraded={}",
                summary.collection,
                summary.selected_docs,
                summary.embedded_docs,
                summary.pending_before,
                summary.pending_after,
                summary.skip_reason,
                summary.degraded
            )
        }
        Err(err) => format!("degraded error={err}"),
    }
}

fn resolve_hot_collection_lifecycle_policy() -> (
    MoonHotCollectionLifecycleMode,
    MoonHotCollectionLifecycleCommandMode,
    Option<String>,
) {
    match load_config() {
        Ok(cfg) => (
            cfg.hot_collection.lifecycle_mode,
            cfg.hot_collection.lifecycle_command_mode,
            None,
        ),
        Err(err) => (
            MoonHotCollectionLifecycleMode::Degrade,
            MoonHotCollectionLifecycleCommandMode::Primary,
            Some(format!(
                "config-load-failed-defaulted err={}",
                crate::moon::util::truncate_with_ellipsis(&format!("{err:#}"), 140)
            )),
        ),
    }
}

fn run_hot_collection_lifecycle(
    paths: &MoonPaths,
    state: &mut MoonState,
    lifecycle_mode: MoonHotCollectionLifecycleMode,
    active_session_id: &str,
    previous_session_id: Option<&str>,
    command_mode: MoonHotCollectionLifecycleCommandMode,
) -> HotCollectionLifecycleRun {
    let probe = qmd::probe_collection_lifecycle_capability(&paths.qmd_bin, command_mode);
    if let Some(summary) =
        hot_collection_lifecycle_probe_summary(lifecycle_mode, &probe, command_mode)
    {
        return summary;
    }

    let active_collection = hot_embed_collection_for_session(active_session_id);
    let active_collection_dir = hot_projection_dir_for_collection(paths, &active_collection);
    let mut degraded = false;
    let mut details = Vec::new();
    details.push(format!(
        "status=active capability={} note={}",
        probe.capability.as_str(),
        probe.note
    ));

    match qmd::collection_create(
        &paths.qmd_bin,
        &active_collection,
        &active_collection_dir,
        command_mode,
        Some(HOT_COLLECTION_LIFECYCLE_TIMEOUT_SECS),
    ) {
        Ok(result) => {
            details.push(format!(
                "register=ok cmd=`{}` fallback={}",
                result.command, result.used_fallback
            ));
        }
        Err(err) => {
            degraded = true;
            details.push(format!(
                "register=degraded error={}",
                crate::moon::util::truncate_with_ellipsis(&format!("{err:#}"), 220)
            ));
        }
    }

    let tracked_at = now_epoch_secs().unwrap_or(0);
    state
        .managed_hot_collections
        .insert(active_collection.clone(), tracked_at);

    let mut stale_collections = state
        .managed_hot_collections
        .keys()
        .filter(|collection| *collection != &active_collection)
        .cloned()
        .collect::<Vec<_>>();
    if let Some(previous) = previous_session_id
        && previous != active_session_id
    {
        let previous_collection = hot_embed_collection_for_session(previous);
        if previous_collection != active_collection
            && !stale_collections.contains(&previous_collection)
        {
            stale_collections.push(previous_collection);
        }
    }
    stale_collections.sort();
    stale_collections.dedup();

    if stale_collections.is_empty() {
        details.push("drop=none".to_string());
    } else {
        let mut dropped = 0usize;
        let mut failed = 0usize;
        for collection_name in stale_collections {
            match qmd::collection_drop(
                &paths.qmd_bin,
                &collection_name,
                command_mode,
                Some(HOT_COLLECTION_LIFECYCLE_TIMEOUT_SECS),
            ) {
                Ok(_) => {
                    dropped += 1;
                    state.managed_hot_collections.remove(&collection_name);
                    remove_embedded_projection_collection(state, &collection_name);
                }
                Err(err) => {
                    degraded = true;
                    failed += 1;
                    details.push(format!(
                        "drop.{}=degraded error={}",
                        collection_name,
                        crate::moon::util::truncate_with_ellipsis(&format!("{err:#}"), 220)
                    ));
                }
            }
        }
        details.push(format!("drop.ok={dropped} drop.failed={failed}"));
    }

    details.push(format!("degraded={degraded}"));
    HotCollectionLifecycleRun {
        summary: details.join(" "),
        degraded,
    }
}

#[cfg(test)]
mod tests {
    use super::{CheckpointOptions, PressureSnapshot, evaluate_cleanse_need, run_checkpoint};
    use crate::moon::paths::MoonPaths;
    use crate::moon::state::load;
    use serde_json::json;
    use std::fs;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::Path;
    use std::sync::Mutex;
    use std::thread;
    use tempfile::tempdir;

    static TEST_ENV_LOCK: Mutex<()> = Mutex::new(());

    struct ScopedEnvVar {
        key: &'static str,
        previous: Option<String>,
    }

    impl ScopedEnvVar {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var(key).ok();
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, previous }
        }
    }

    impl Drop for ScopedEnvVar {
        fn drop(&mut self) {
            if let Some(previous) = &self.previous {
                unsafe {
                    std::env::set_var(self.key, previous);
                }
            } else {
                unsafe {
                    std::env::remove_var(self.key);
                }
            }
        }
    }

    fn start_fake_openai_compatible_server(
        response_body: &str,
    ) -> (thread::JoinHandle<()>, String) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake server");
        let addr = listener.local_addr().expect("local addr");
        let body = response_body.to_string();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            stream
                .set_read_timeout(Some(std::time::Duration::from_millis(500)))
                .expect("read timeout");
            let mut buf = [0u8; 4096];
            loop {
                match stream.read(&mut buf) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(_) => break,
                }
            }

            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .expect("write response");
        });
        (handle, format!("http://{}", addr))
    }

    fn test_paths(root: &Path) -> MoonPaths {
        MoonPaths {
            moon_home: root.to_path_buf(),
            raw_dir: root.join("raw"),
            mds_dir: root.join("mds"),
            mlib_dir: root.join("mlib"),
            cleanse_dir: root.join("cleanse"),
            memory_dir: root.join("memory"),
            memory_file: root.join("MEMORY.md"),
            logs_dir: root.join("logs"),
            context_engine_dir: root.join("mce"),
            context_packet_dir: root.join("mcp"),
            openclaw_sessions_dir: root.join("sessions"),
            qmd_bin: root.join("bin/qmd"),
            qmd_db: root.join("qmd.sqlite"),
            qmd_config_dir: root.join("qmd-config"),
            moon_home_is_explicit: true,
        }
    }

    fn write_fake_qmd(bin_path: &std::path::Path) {
        let script = r#"#!/usr/bin/env bash
set -euo pipefail

if [[ "${1:-}" == "embed" && "${2:-}" == "--help" ]]; then
  echo "Usage: qmd embed <collection> --max-docs <n>"
  exit 0
fi

if [[ "${1:-}" == "embed" ]]; then
  exit 0
fi

	if [[ "${1:-}" == "collection" ]]; then
	  if [[ "${2:-}" == "--help" ]]; then
	    echo "Commands: add remove show"
	  fi
	  exit 0
	fi

exit 0
"#;
        if let Some(parent) = bin_path.parent() {
            fs::create_dir_all(parent).expect("mkdir qmd bin dir");
        }
        fs::write(bin_path, script).expect("write fake qmd");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(bin_path).expect("metadata").permissions();
            perms.set_mode(0o755);
            fs::set_permissions(bin_path, perms).expect("chmod");
        }
    }

    #[test]
    fn checkpoint_records_and_assembles_without_cleanse_below_trigger() {
        let tmp = tempdir().expect("tempdir");
        let moon_home = tmp.path().join("moon-home");
        let paths = test_paths(&moon_home);
        write_fake_qmd(&paths.qmd_bin);
        fs::create_dir_all(&paths.openclaw_sessions_dir).expect("mkdir sessions");

        let source = paths.openclaw_sessions_dir.join("session-a.jsonl");
        let user = json!({
            "message": {
                "role": "user",
                "content": [{"type":"text","text":"Keep the checkpoint path simple."}]
            }
        });
        let assistant = json!({
            "message": {
                "role": "assistant",
                "content": [{"type":"text","text":"Start with record and assemble before fallback."}]
            }
        });
        fs::write(&source, format!("{user}\n{assistant}\n")).expect("write source");

        let output = run_checkpoint(
            &paths,
            &CheckpointOptions {
                source_path: Some(source.display().to_string()),
                session_id: Some("session-a".to_string()),
                pressure: Some(PressureSnapshot {
                    used_tokens: 59_000,
                    max_tokens: 200_000,
                }),
                force_cleanse: false,
                replay_has_compaction_summary: false,
            },
        )
        .expect("run checkpoint");

        assert_eq!(output.session_id, "session-a");
        assert!(output.record_target_path.ends_with("/raw/session-a.jsonl"));
        assert!(output.cleanse_summary_path.is_none());
        assert!(output.cleanse_reason.starts_with("below-trigger"));
        assert!(output.assembly_output_path.ends_with("/mce/session-a.md"));
        assert!(
            output
                .context_packet_output_path
                .as_deref()
                .is_some_and(|path| path.ends_with("/mcp/session-a.md"))
        );
        assert!(
            output
                .assembly
                .content
                .contains("Keep the checkpoint path simple.")
        );
        assert!(
            output
                .context_packet
                .as_ref()
                .is_some_and(|packet| packet.content.contains("# Moon Active Context"))
        );
        assert!(output.assembly.content.contains("- cleanse_summary: none"));
        assert!(
            output
                .assembly
                .content
                .contains("## Embedding Index Anchor")
        );
        assert!(std::path::Path::new(&output.assembly_output_path).is_file());
        assert!(crate::moon::state::hot_projection_path_for_session(&paths, "session-a").is_file());

        let state = load(&paths).expect("load state");
        assert_eq!(state.last_session_id.as_deref(), Some("session-a"));
        assert_eq!(state.last_usage_ratio, Some(59_000.0 / 200_000.0));
    }

    #[test]
    fn checkpoint_runs_cleanse_and_assembles_when_forced() {
        let _lock = TEST_ENV_LOCK.lock().expect("env lock");
        let tmp = tempdir().expect("tempdir");
        let moon_home = tmp.path().join("moon-home");
        let paths = test_paths(&moon_home);
        write_fake_qmd(&paths.qmd_bin);
        fs::create_dir_all(&paths.openclaw_sessions_dir).expect("mkdir sessions");

        let source = paths.openclaw_sessions_dir.join("session-b.jsonl");
        let user = json!({
            "message": {
                "role": "user",
                "content": [{"type":"text","text":"Force a cleanse and assemble pass."}]
            }
        });
        let assistant = json!({
            "message": {
                "role": "assistant",
                "content": [{"type":"text","text":"The primary path should stay MOON-owned."}]
            }
        });
        fs::write(&source, format!("{user}\n{assistant}\n")).expect("write source");

        let response_body = r##"{"choices":[{"message":{"content":"# Cleanse Summary\n## Current Goal\n- Keep the primary path MOON-owned.\n## Decisions\n- Keep the primary path MOON-owned.\n## Open Tasks\n- Wire the checkpoint runner into the final control surface.\n## Risks / Blockers\n- Do not reintroduce watcher-first ownership."}}]}"##;
        let (server_handle, base_url) = start_fake_openai_compatible_server(response_body);
        let _provider = ScopedEnvVar::set("MOON_CLEANSE_PROVIDER", "openai-compatible");
        let _model = ScopedEnvVar::set("MOON_CLEANSE_MODEL", "test-cleanse");
        let _base_url = ScopedEnvVar::set("AI_BASE_URL", &base_url);
        let _api_key = ScopedEnvVar::set("AI_API_KEY", "test-key");

        let output = run_checkpoint(
            &paths,
            &CheckpointOptions {
                source_path: Some(source.display().to_string()),
                session_id: Some("session-b".to_string()),
                pressure: None,
                force_cleanse: true,
                replay_has_compaction_summary: false,
            },
        )
        .expect("run checkpoint");

        server_handle.join().expect("join fake server");

        let cleanse_path = output
            .cleanse_summary_path
            .clone()
            .expect("cleanse summary path");
        assert_eq!(output.cleanse_reason, "forced");
        assert!(cleanse_path.ends_with("/cleanse/session-b.md"));
        assert!(output.assembly_output_path.ends_with("/mce/session-b.md"));
        assert!(
            output
                .context_packet_output_path
                .as_deref()
                .is_some_and(|path| path.ends_with("/mcp/session-b.md"))
        );
        assert!(
            output
                .assembly
                .content
                .contains("Keep the primary path MOON-owned.")
        );
        assert!(
            output
                .context_packet
                .as_ref()
                .is_some_and(|packet| packet.content.contains("# Moon Active Context"))
        );
        assert!(
            output
                .assembly
                .content
                .contains("Wire the checkpoint runner into the final control surface.")
        );
        assert!(
            output
                .assembly
                .content
                .contains("## Embedding Index Anchor")
        );
        assert!(std::path::Path::new(&output.assembly_output_path).is_file());
        assert!(crate::moon::state::hot_projection_path_for_session(&paths, "session-b").is_file());

        let state = load(&paths).expect("load state");
        assert_eq!(state.last_session_id.as_deref(), Some("session-b"));
        assert!(state.last_compaction_trigger_epoch_secs.is_some());
        assert!(state.pending_embed_collections.is_empty());
    }

    #[test]
    fn evaluate_cleanse_need_uses_context_window_tokens_and_cleanse_ratios() {
        let _lock = TEST_ENV_LOCK.lock().expect("env lock");
        let tmp = tempdir().expect("tempdir");
        let moon_home = tmp.path().join("moon-home");
        fs::create_dir_all(&moon_home).expect("mkdir moon home");
        fs::write(
            moon_home.join("moon.toml"),
            r#"[context]
window_mode = "fixed"
window_tokens = 20000
compaction_authority = "moon"
cleanse_trigger_ratio = 0.50
cleanse_emergency_ratio = 0.90
"#,
        )
        .expect("write moon.toml");
        let _home = ScopedEnvVar::set("MOON_HOME", &moon_home.display().to_string());

        let (should_cleanse, reason, usage_ratio) = evaluate_cleanse_need(
            Some(PressureSnapshot {
                used_tokens: 11_000,
                max_tokens: 20_000,
            }),
            false,
        );

        assert!(should_cleanse);
        assert_eq!(usage_ratio, Some(11_000.0 / 20_000.0));
        assert!(reason.contains("trigger-used-tokens>=10000"), "{reason}");
        assert!(reason.contains("window_tokens=20000"), "{reason}");
    }
}
