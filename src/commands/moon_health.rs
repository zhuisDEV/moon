use crate::commands::CommandReport;
use crate::moon::assemble::output_path as assembly_output_path;
use crate::moon::config::{
    MoonHotCollectionLifecycleMode, resolve_hot_collection_lifecycle_policy_for_diagnostics,
};
use crate::moon::daemon_lock::{daemon_lock_path, read_daemon_lock_payload};
use crate::moon::paths::resolve_paths;
use crate::moon::qmd;
use crate::moon::state::{self, MoonState};
use crate::moon::util::now_epoch_secs;
use anyhow::Result;
use std::fs;
use std::io::Write;

const DEFAULT_MAX_CYCLE_AGE_SECS: u64 = 600;

fn max_cycle_age_secs() -> u64 {
    std::env::var("MOON_HEALTH_MAX_CYCLE_AGE_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_MAX_CYCLE_AGE_SECS)
}

fn check_state_file(
    paths: &crate::moon::paths::MoonPaths,
    report: &mut CommandReport,
) -> Option<MoonState> {
    let state_path = state::state_file_path(paths);
    report.detail(format!("state.file={}", state_path.display()));

    let state_exists = state_path.exists();

    if let Some(parent) = state_path.parent() {
        if let Err(err) = fs::create_dir_all(parent) {
            report.issue(format!("state.dir=unwritable ({err})"));
            return None;
        }
        if state_exists {
            let writable = fs::OpenOptions::new()
                .append(true)
                .open(&state_path)
                .and_then(|mut f| f.write_all(b""));
            if let Err(err) = writable {
                report.issue(format!("state.file=unwritable ({err})"));
                return None;
            }
        } else {
            let probe = parent.join(".moon-health-write-probe");
            let writable = fs::write(&probe, b"probe").and_then(|_| fs::remove_file(&probe));
            if let Err(err) = writable {
                report.issue(format!("state.dir=unwritable ({err})"));
                return None;
            }
        }
    }
    report.detail("state.file=writable".to_string());

    if !state_exists {
        report.detail("state.file=not_found (will be created on first cycle)".to_string());
        return None;
    }

    let raw = match fs::read_to_string(&state_path) {
        Ok(raw) => raw,
        Err(err) => {
            report.issue(format!("state.file=unreadable ({err})"));
            return None;
        }
    };

    let parsed: MoonState = match serde_json::from_str(&raw) {
        Ok(state) => state,
        Err(err) => {
            report.issue(format!("state.file=corrupt ({err})"));
            return None;
        }
    };

    report.detail("state.file=parse_ok".to_string());
    if parsed.last_heartbeat_epoch_secs == 0 {
        report.issue("state.last_heartbeat=missing".to_string());
        return Some(parsed);
    }

    let now = now_epoch_secs().unwrap_or(parsed.last_heartbeat_epoch_secs);
    let age = now.saturating_sub(parsed.last_heartbeat_epoch_secs);
    report.detail(format!("state.last_heartbeat_age_secs={age}"));

    let max_age = max_cycle_age_secs();
    if age > max_age {
        report.issue(format!(
            "state.last_heartbeat=stale age_secs={age} max_allowed_secs={max_age}"
        ));
    } else {
        report.detail(format!(
            "state.last_heartbeat=fresh max_allowed_secs={max_age}"
        ));
    }

    Some(parsed)
}

fn check_latest_assembly(
    paths: &crate::moon::paths::MoonPaths,
    state: Option<&MoonState>,
    report: &mut CommandReport,
) {
    let Some(state) = state else {
        report.detail("context_engine.latest_output=unknown (state unavailable)".to_string());
        return;
    };
    let Some(session_id) = state.last_assembly_session_id.as_deref() else {
        report.detail("context_engine.latest_output=unknown (no assembly recorded)".to_string());
        return;
    };

    let output_path = assembly_output_path(paths, session_id);
    if output_path.exists() {
        report.detail(format!(
            "context_engine.latest_output=ok ({})",
            output_path.display()
        ));
    } else {
        report.issue(format!(
            "context_engine.latest_output=missing ({})",
            output_path.display()
        ));
    }
}

pub fn run() -> Result<CommandReport> {
    let mut report = CommandReport::new("health");
    let paths = resolve_paths()?;
    qmd::install_runtime_env(&paths);
    let (lifecycle_mode, lifecycle_command_mode, lifecycle_mode_note) =
        resolve_hot_collection_lifecycle_policy_for_diagnostics();
    let lifecycle_probe =
        qmd::probe_collection_lifecycle_capability(&paths.qmd_bin, lifecycle_command_mode);

    report.detail(format!("moon_home={}", paths.moon_home.display()));
    report.detail(format!(
        "hot_collection.lifecycle_mode={}",
        lifecycle_mode.as_str()
    ));
    report.detail(format!(
        "hot_collection.lifecycle_command_mode={}",
        lifecycle_command_mode.as_str()
    ));
    if let Some(note) = lifecycle_mode_note {
        report.issue(format!(
            "hot_collection.lifecycle_mode_note={}",
            crate::moon::util::truncate_with_ellipsis(&note, 220)
        ));
    }
    report.detail(format!(
        "hot_collection.lifecycle_capability={}",
        lifecycle_probe.capability.as_str()
    ));
    report.detail(format!(
        "hot_collection.lifecycle_note={}",
        &lifecycle_probe.note
    ));
    if lifecycle_mode == MoonHotCollectionLifecycleMode::Disabled {
        report.detail(format!(
            "hot collection lifecycle disabled by config ({})",
            &lifecycle_probe.note
        ));
    } else if lifecycle_mode == MoonHotCollectionLifecycleMode::Degrade
        && lifecycle_probe.capability == qmd::CollectionLifecycleCapability::Missing
    {
        report.issue(format!(
            "hot collection lifecycle running in degraded mode; qmd lifecycle support missing ({})",
            &lifecycle_probe.note
        ));
    }

    // Check paths
    for (name, path) in [
        ("raw_dir", &paths.raw_dir),
        ("mds_dir", &paths.mds_dir),
        ("mlib_dir", &paths.mlib_dir),
        ("cleanse_dir", &paths.cleanse_dir),
        ("logs_dir", &paths.logs_dir),
        ("context_engine_dir", &paths.context_engine_dir),
    ] {
        if path.exists() {
            report.detail(format!("path.{name}=ok"));
        } else {
            report.issue(format!("path.{name}=missing ({})", path.display()));
        }
    }
    if paths.context_packet_dir.exists() {
        report.detail("path.context_packet_dir=ok".to_string());
    } else {
        report.detail(format!(
            "path.context_packet_dir=not_created_yet ({})",
            paths.context_packet_dir.display()
        ));
    }

    // Check daemon lock
    let lock_path = daemon_lock_path(&paths);
    if lock_path.exists() {
        match read_daemon_lock_payload(&paths) {
            Ok(Some(payload)) => {
                report.detail("daemon.lock=found".to_string());
                report.detail(format!("daemon.pid={}", payload.pid));
                if payload.started_at_epoch_secs > 0 {
                    report.detail(format!(
                        "daemon.started_at_epoch_secs={}",
                        payload.started_at_epoch_secs
                    ));
                }
                if !payload.moon_home.trim().is_empty() {
                    report.detail(format!("daemon.moon_home={}", payload.moon_home.trim()));
                }

                if crate::moon::util::pid_alive(payload.pid) {
                    report.detail("daemon.process=alive".to_string());
                } else {
                    report.issue("daemon.process=dead (stale lock)".to_string());
                }

                if !payload.build_uuid.trim().is_empty() {
                    let current_uuid = env!("BUILD_UUID");
                    if payload.build_uuid == current_uuid {
                        report.detail("daemon.build_match=ok".to_string());
                    } else {
                        report.issue(format!(
                            "daemon.build_mismatch=found (lock={} current={})",
                            payload.build_uuid, current_uuid
                        ));
                    }
                } else {
                    report.issue("daemon.build_uuid=missing".to_string());
                }
            }
            Ok(None) => {
                report.issue("daemon.lock=corrupt (empty payload)".to_string());
            }
            Err(err) => {
                report.issue(format!("daemon.lock=corrupt ({err})"));
            }
        }
    } else {
        report.detail("daemon.lock=not_found (daemon likely not running)".to_string());
    }

    let state = check_state_file(&paths, &mut report);
    if let Some(state) = state.as_ref() {
        report.detail(format!(
            "state.managed_hot_collections={}",
            state.managed_hot_collections.len()
        ));
    }
    check_latest_assembly(&paths, state.as_ref(), &mut report);
    if lifecycle_mode == MoonHotCollectionLifecycleMode::Strict
        && lifecycle_probe.capability == qmd::CollectionLifecycleCapability::Missing
    {
        report.issue(format!(
            "hot collection lifecycle strict mode requires qmd lifecycle support ({})",
            &lifecycle_probe.note
        ));
    }

    Ok(report)
}
