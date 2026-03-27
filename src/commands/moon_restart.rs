use anyhow::Result;
use std::env;
use std::process::{Command, Stdio};

use crate::commands::CommandReport;
use crate::commands::moon_stop;

fn is_dev_build_path(path: &std::path::Path) -> bool {
    let exe_str = path.display().to_string();
    exe_str.contains("target/debug")
        || exe_str.contains("target/release")
        || exe_str.contains("target\\debug")
        || exe_str.contains("target\\release")
}

#[cfg(target_os = "macos")]
fn start_via_launchd(report: &mut CommandReport) -> Result<bool> {
    let plist_path = crate::moon::launchd::plist_path()?;
    if !plist_path.exists() {
        report.detail(format!(
            "launchd.start=skipped reason=plist_missing path={}",
            plist_path.display()
        ));
        return Ok(false);
    }

    let bootstrap_out = crate::moon::launchd::bootstrap_service()?;
    if !bootstrap_out.status.success() {
        anyhow::bail!(
            "launchctl bootstrap failed: {}",
            crate::moon::launchd::summarize_command_failure(&bootstrap_out)
        );
    }
    report.detail("launchd.bootstrap=ok".to_string());

    let kickstart_out = crate::moon::launchd::kickstart_service()?;
    if !kickstart_out.status.success() {
        anyhow::bail!(
            "launchctl kickstart failed: {}",
            crate::moon::launchd::summarize_command_failure(&kickstart_out)
        );
    }
    report.detail("launchd.kickstart=ok".to_string());
    Ok(true)
}

#[cfg(not(target_os = "macos"))]
fn start_via_launchd(_report: &mut CommandReport) -> Result<bool> {
    Ok(false)
}

fn spawn_background_daemon(report: &mut CommandReport) -> Result<()> {
    let current_exe = env::current_exe()?;
    let paths = crate::moon::paths::resolve_paths()?;
    let child = Command::new(&current_exe)
        .arg("watch")
        .arg("--daemon")
        .current_dir(&paths.moon_home)
        .env("MOON_HOME", paths.moon_home.display().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    report.detail(format!("daemon.spawned.pid={}", child.id()));
    Ok(())
}

pub fn run() -> Result<CommandReport> {
    let mut report = CommandReport::new("restart");
    let current_exe = env::current_exe()?;
    if is_dev_build_path(&current_exe) {
        report.issue(
            "CRITICAL: Running the background daemon from a development binary is disabled for stability.",
        );
        report.issue("Please install the binary to your path first: `cargo install --path .`");
        report.issue("Then rerun `moon restart` from the installed binary.");
        return Ok(report);
    }

    crate::commands::align_openclaw_plugin_state(&mut report, "restart");

    report.detail("stopping existing watcher daemon".to_string());
    let stop_report = moon_stop::run()?;
    let stop_ok = stop_report.ok;
    report.merge(stop_report);
    if !stop_ok {
        report.issue("restart aborted: stop failed");
        return Ok(report);
    }

    report.detail("starting new watcher daemon".to_string());
    if !start_via_launchd(&mut report)? {
        spawn_background_daemon(&mut report)?;
    }

    Ok(report)
}
