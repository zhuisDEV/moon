use anyhow::Result;
#[cfg(target_os = "macos")]
use std::env;
#[cfg(target_os = "macos")]
use std::fs;

use crate::commands::CommandReport;
use crate::commands::status::report_openclaw_memory_contract;
use crate::moon::assemble::output_path as assembly_output_path;
use crate::moon::config::{
    MoonHotCollectionLifecycleMode, SECRET_ENV_KEYS, masked_env_secret,
    resolve_hot_collection_lifecycle_policy_for_diagnostics,
};
use crate::moon::daemon_lock::{daemon_lock_path, read_daemon_lock_payload};
use crate::moon::paths::resolve_paths;
use crate::moon::qmd;
use crate::moon::state::{load, state_file_path};
use crate::openclaw::config::{
    inspect_moon_owned_memory_contract, read_config_value as read_openclaw_config_value,
};
use crate::openclaw::paths::resolve_paths as resolve_openclaw_paths;

fn optional_text(value: Option<&str>) -> &str {
    value.unwrap_or("none")
}

fn optional_u64(value: Option<u64>) -> String {
    value
        .map(|v| v.to_string())
        .unwrap_or_else(|| "none".to_string())
}

fn optional_f64(value: Option<f64>) -> String {
    value
        .map(|v| v.to_string())
        .unwrap_or_else(|| "none".to_string())
}

pub fn run() -> Result<CommandReport> {
    let paths = resolve_paths()?;
    qmd::install_runtime_env(&paths);
    let state = load(&paths)?;
    let (lifecycle_mode, lifecycle_command_mode, lifecycle_mode_note) =
        resolve_hot_collection_lifecycle_policy_for_diagnostics();
    let lifecycle_probe =
        qmd::probe_collection_lifecycle_capability(&paths.qmd_bin, lifecycle_command_mode);
    let mut report = CommandReport::new("status");

    report.detail(format!("moon_home={}", paths.moon_home.display()));
    report.detail(format!("raw_dir={}", paths.raw_dir.display()));
    report.detail(format!("mds_dir={}", paths.mds_dir.display()));
    report.detail(format!("mlib_dir={}", paths.mlib_dir.display()));
    report.detail(format!("cleanse_dir={}", paths.cleanse_dir.display()));
    report.detail(format!("memory_dir={}", paths.memory_dir.display()));
    report.detail(format!("memory_file={}", paths.memory_file.display()));
    report.detail(format!("logs_dir={}", paths.logs_dir.display()));
    report.detail(format!(
        "context_engine_dir={}",
        paths.context_engine_dir.display()
    ));
    report.detail(format!(
        "context_packet_dir={}",
        paths.context_packet_dir.display()
    ));
    report.detail(format!("state_file={}", state_file_path(&paths).display()));
    report.detail(format!(
        "state.last_session_id={}",
        optional_text(state.last_session_id.as_deref())
    ));
    report.detail(format!(
        "state.last_usage_ratio={}",
        optional_f64(state.last_usage_ratio)
    ));
    report.detail(format!(
        "state.last_compaction_trigger_epoch_secs={}",
        optional_u64(state.last_compaction_trigger_epoch_secs)
    ));
    report.detail(format!(
        "state.last_provider={}",
        optional_text(state.last_provider.as_deref())
    ));
    report.detail(format!(
        "state.last_assembly_session_id={}",
        optional_text(state.last_assembly_session_id.as_deref())
    ));
    report.detail(format!(
        "state.last_assembly_epoch_secs={}",
        optional_u64(state.last_assembly_epoch_secs)
    ));
    report.detail(format!(
        "state.last_context_packet_session_id={}",
        optional_text(state.last_context_packet_session_id.as_deref())
    ));
    report.detail(format!(
        "state.last_context_packet_epoch_secs={}",
        optional_u64(state.last_context_packet_epoch_secs)
    ));
    if let Some(session_id) = state.last_assembly_session_id.as_deref() {
        let latest_output = assembly_output_path(&paths, session_id);
        report.detail(format!(
            "context_engine.latest_output_path={}",
            latest_output.display()
        ));
        report.detail(format!(
            "context_engine.latest_output_exists={}",
            latest_output.exists()
        ));
    } else {
        report.detail("context_engine.latest_output_path=none".to_string());
        report.detail("context_engine.latest_output_exists=false".to_string());
    }
    report.detail(format!(
        "openclaw_sessions_dir={}",
        paths.openclaw_sessions_dir.display()
    ));
    let openclaw_paths = resolve_openclaw_paths()?;
    let openclaw_cfg = read_openclaw_config_value(&openclaw_paths)?;
    report.detail(format!(
        "openclaw_config_path={}",
        openclaw_paths.config_path.display()
    ));
    report_openclaw_memory_contract(
        &inspect_moon_owned_memory_contract(&openclaw_cfg),
        &mut report,
    );
    report.detail(format!("qmd_bin={}", paths.qmd_bin.display()));
    report.detail(format!("qmd_db={}", paths.qmd_db.display()));
    report.detail(format!(
        "hot_collection.lifecycle_mode={}",
        lifecycle_mode.as_str()
    ));
    report.detail(format!(
        "hot_collection.lifecycle_command_mode={}",
        lifecycle_command_mode.as_str()
    ));
    if let Some(note) = lifecycle_mode_note {
        report.detail(format!(
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
        report.detail(format!(
            "hot collection lifecycle running in degraded mode; qmd lifecycle support missing ({})",
            &lifecycle_probe.note
        ));
    }
    report.detail(format!(
        "state.managed_hot_collections={}",
        state.managed_hot_collections.len()
    ));
    for key in SECRET_ENV_KEYS {
        report.detail(format!("secret.{key}={}", masked_env_secret(key)));
    }
    for issue in crate::moon::fs_security::runtime_secret_permission_issues(&paths)? {
        report.issue(issue);
    }
    report_daemon_runtime(&paths, &mut report);

    if !paths.raw_dir.exists() {
        report.issue(format!("missing raw dir ({})", paths.raw_dir.display()));
    }
    if !paths.mds_dir.exists() {
        report.issue(format!("missing mds dir ({})", paths.mds_dir.display()));
    }
    if !paths.mlib_dir.exists() {
        report.issue(format!("missing mlib dir ({})", paths.mlib_dir.display()));
    }
    if !paths.cleanse_dir.exists() {
        report.issue(format!(
            "missing cleanse dir ({})",
            paths.cleanse_dir.display()
        ));
    }
    if !paths.memory_dir.exists() {
        report.issue(format!(
            "missing daily memory dir ({})",
            paths.memory_dir.display()
        ));
    }
    if !paths.logs_dir.exists() {
        report.issue(format!(
            "missing moon log dir ({})",
            paths.logs_dir.display()
        ));
    }
    if !paths.context_engine_dir.exists() {
        report.issue(format!(
            "missing context-engine dir ({})",
            paths.context_engine_dir.display()
        ));
    }
    if !paths.context_packet_dir.exists() {
        report.detail(format!(
            "context-packet dir not created yet ({})",
            paths.context_packet_dir.display()
        ));
    }
    if !paths.memory_file.exists() {
        report.issue(format!(
            "missing long-term memory file ({})",
            paths.memory_file.display()
        ));
    }
    if !paths.openclaw_sessions_dir.exists() {
        report.issue(format!(
            "missing OpenClaw sessions dir ({})",
            paths.openclaw_sessions_dir.display()
        ));
    }
    if !paths.qmd_bin.exists() {
        report.issue(format!("missing qmd binary ({})", paths.qmd_bin.display()));
    }
    if lifecycle_mode == MoonHotCollectionLifecycleMode::Strict
        && lifecycle_probe.capability == qmd::CollectionLifecycleCapability::Missing
    {
        report.issue(format!(
            "hot collection lifecycle strict mode requires qmd collection lifecycle support ({})",
            &lifecycle_probe.note
        ));
    }

    Ok(report)
}

fn report_daemon_runtime(moon_paths: &crate::moon::paths::MoonPaths, report: &mut CommandReport) {
    let lock_path = daemon_lock_path(moon_paths);
    report.detail(format!("daemon.lock_path={}", lock_path.display()));

    let autostart_expected = daemon_autostart_expected_for_workspace(moon_paths);
    report.detail(format!("daemon.autostart_expected={autostart_expected}"));

    match read_daemon_lock_payload(moon_paths) {
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

            let alive = crate::moon::util::pid_alive(payload.pid);
            report.detail(format!("daemon.process_alive={alive}"));
            if !alive {
                report.issue(format!(
                    "daemon lock is stale: pid {} is not running; run `moon restart`",
                    payload.pid
                ));
            }
        }
        Ok(None) => {
            report.detail("daemon.lock=missing".to_string());
            if autostart_expected {
                report.issue(
                    "daemon lock missing while launchd autostart is configured; run `moon restart`"
                        .to_string(),
                );
            }
        }
        Err(err) => {
            report.issue(format!(
                "failed to inspect daemon lock {}: {err:#}",
                lock_path.display()
            ));
        }
    }
}

#[cfg(target_os = "macos")]
fn daemon_autostart_expected_for_workspace(moon_paths: &crate::moon::paths::MoonPaths) -> bool {
    let Ok(current_exe) = env::current_exe() else {
        return false;
    };
    if is_dev_build_path(&current_exe) {
        return false;
    }

    let Ok(plist_path) = crate::moon::launchd::plist_path() else {
        return false;
    };
    let Ok(plist) = fs::read_to_string(&plist_path) else {
        return false;
    };
    let Some(working_dir) = extract_launchd_working_directory(&plist) else {
        return false;
    };

    working_dir == moon_paths.moon_home.display().to_string()
}

#[cfg(not(target_os = "macos"))]
fn daemon_autostart_expected_for_workspace(_moon_paths: &crate::moon::paths::MoonPaths) -> bool {
    false
}

#[cfg(target_os = "macos")]
fn extract_launchd_working_directory(plist: &str) -> Option<String> {
    let marker = "<key>WorkingDirectory</key><string>";
    let start = plist.find(marker)? + marker.len();
    let end = plist[start..].find("</string>")?;
    Some(plist[start..start + end].to_string())
}

#[cfg(target_os = "macos")]
fn is_dev_build_path(path: &std::path::Path) -> bool {
    let normalized = path.display().to_string();
    normalized.contains("target/debug")
        || normalized.contains("target/release")
        || normalized.contains("target\\debug")
        || normalized.contains("target\\release")
}
