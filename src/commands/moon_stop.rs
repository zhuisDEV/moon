use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use crate::commands::CommandReport;
use crate::moon::daemon_lock::{daemon_lock_path, read_daemon_lock_payload};
use crate::moon::paths::resolve_paths;
use crate::moon::util::run_command_with_optional_timeout;

const STOP_TIMEOUT: Duration = Duration::from_secs(8);
const STOP_POLL_INTERVAL: Duration = Duration::from_millis(100);
const COMMAND_TIMEOUT_SECS: u64 = 30;

fn process_alive(pid: u32) -> Result<bool> {
    let mut kill_cmd = Command::new("kill");
    kill_cmd.arg("-0").arg(pid.to_string());
    let kill_out = run_command_with_optional_timeout(&mut kill_cmd, Some(COMMAND_TIMEOUT_SECS))
        .context("failed to probe process state with `kill -0`")?;
    if !kill_out.status.success() {
        return Ok(false);
    }

    let mut ps_cmd = Command::new("ps");
    ps_cmd.arg("-p").arg(pid.to_string()).arg("-o").arg("stat=");
    let ps_out = run_command_with_optional_timeout(&mut ps_cmd, Some(COMMAND_TIMEOUT_SECS))
        .context("failed to inspect process state with `ps`")?;

    if !ps_out.status.success() {
        return Ok(false);
    }

    let proc_state = String::from_utf8_lossy(&ps_out.stdout).trim().to_string();
    if proc_state.starts_with('Z') {
        return Ok(false);
    }

    Ok(true)
}

fn process_command_line(pid: u32) -> Result<String> {
    let mut ps_cmd = Command::new("ps");
    ps_cmd
        .arg("-p")
        .arg(pid.to_string())
        .arg("-o")
        .arg("command=");
    let output = run_command_with_optional_timeout(&mut ps_cmd, Some(COMMAND_TIMEOUT_SECS))
        .context("failed to inspect process command line with `ps`")?;
    if !output.status.success() {
        return Ok(String::new());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn send_sigterm(pid: u32) -> Result<()> {
    let mut kill_cmd = Command::new("kill");
    kill_cmd.arg("-TERM").arg(pid.to_string());
    let out = run_command_with_optional_timeout(&mut kill_cmd, Some(COMMAND_TIMEOUT_SECS))
        .context("failed to send SIGTERM with `kill -TERM`")?;

    if out.status.success() {
        return Ok(());
    }

    if process_alive(pid)? {
        anyhow::bail!("`kill -TERM {pid}` failed and process is still alive");
    }

    Ok(())
}

fn cleanup_lock_file(lock_path: &Path, report: &mut CommandReport) {
    match fs::remove_file(lock_path) {
        Ok(()) => report.detail(format!("removed stale daemon lock {}", lock_path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => report.detail(format!(
            "failed to remove daemon lock {}: {}",
            lock_path.display(),
            err
        )),
    }
}

fn is_watcher_command(command_line: &str) -> bool {
    command_line.contains("moon-watch --daemon") || command_line.contains(" watch --daemon")
}

fn is_restart_helper_command(command_line: &str) -> bool {
    command_line.starts_with("moon restart")
        || command_line.contains(" moon restart")
        || command_line.contains("moon --allow-out-of-bounds restart")
}

fn is_managed_moon_process(command_line: &str) -> bool {
    is_watcher_command(command_line) || is_restart_helper_command(command_line)
}

fn command_mentions_moon_home(command_line: &str, moon_home: &str) -> bool {
    command_line.contains(moon_home)
        || command_line.contains(&format!("MOON_HOME={moon_home}"))
        || command_line.contains(&format!("cd {moon_home}"))
}

fn stop_pid(pid: u32, command_line: &str, report: &mut CommandReport) -> Result<bool> {
    if !process_alive(pid)? {
        report.detail(format!("daemon pid {pid} is not running"));
        return Ok(false);
    }

    if !is_managed_moon_process(command_line) {
        report.issue(format!(
            "refusing to stop pid {pid}; command does not match moon watcher daemon: {}",
            if command_line.is_empty() {
                "<unknown>".to_string()
            } else {
                command_line.to_string()
            }
        ));
        return Ok(false);
    }

    send_sigterm(pid)?;
    let deadline = Instant::now() + STOP_TIMEOUT;
    while Instant::now() < deadline {
        if !process_alive(pid)? {
            report.detail(format!("stopped moon watcher daemon pid={pid}"));
            return Ok(true);
        }
        thread::sleep(STOP_POLL_INTERVAL);
    }

    report.issue(format!(
        "timed out waiting for daemon pid {pid} to stop after {}s",
        STOP_TIMEOUT.as_secs()
    ));
    Ok(false)
}

fn list_candidate_processes(moon_home: &str) -> Result<Vec<(u32, String)>> {
    match list_candidate_processes_via_pgrep(moon_home) {
        Ok(processes) => Ok(processes),
        Err(err) if should_fallback_to_ps(&err) => list_candidate_processes_via_ps(moon_home)
            .context("failed to list processes with `ps` after `pgrep` was unavailable"),
        Err(err) => Err(err),
    }
}

fn should_fallback_to_ps(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .map(|io_err| io_err.kind() == std::io::ErrorKind::NotFound)
            .unwrap_or(false)
    })
}

fn parse_pid_command_lines(raw: &str) -> Vec<(u32, String)> {
    let self_pid = std::process::id();
    let mut processes = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let Some(pid_raw) = parts.next() else {
            continue;
        };
        let Some(command_line_raw) = parts.next() else {
            continue;
        };
        let Ok(pid) = pid_raw.trim().parse::<u32>() else {
            continue;
        };
        if pid == self_pid {
            continue;
        }
        let command_line = command_line_raw.trim().to_string();
        if is_managed_moon_process(&command_line) {
            processes.push((pid, command_line));
        }
    }
    processes
}

fn list_candidate_processes_via_pgrep(moon_home: &str) -> Result<Vec<(u32, String)>> {
    let mut cmd = Command::new("pgrep");
    cmd.arg("-f").arg("moon");
    let output = run_command_with_optional_timeout(&mut cmd, Some(COMMAND_TIMEOUT_SECS))
        .context("failed to list processes with `pgrep -f moon`")?;
    if !output.status.success() {
        if output.status.code() == Some(1) {
            return Ok(Vec::new());
        }
        anyhow::bail!("`pgrep -f moon` failed");
    }
    let self_pid = std::process::id();
    let mut processes = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Ok(pid) = line.trim().parse::<u32>() else {
            continue;
        };
        if pid == self_pid {
            continue;
        }
        let command_line = process_command_line(pid)?;
        if is_managed_moon_process(&command_line)
            && command_mentions_moon_home(&command_line, moon_home)
        {
            processes.push((pid, command_line));
        }
    }
    Ok(processes)
}

fn list_candidate_processes_via_ps(moon_home: &str) -> Result<Vec<(u32, String)>> {
    let mut ps_cmd = Command::new("ps");
    ps_cmd.arg("-axo").arg("pid=,command=");
    let output = run_command_with_optional_timeout(&mut ps_cmd, Some(COMMAND_TIMEOUT_SECS))
        .context("failed to list processes with `ps`")?;
    if !output.status.success() {
        anyhow::bail!("`ps -axo pid=,command=` failed");
    }

    Ok(
        parse_pid_command_lines(&String::from_utf8_lossy(&output.stdout))
            .into_iter()
            .filter(|(_, command_line)| command_mentions_moon_home(command_line, moon_home))
            .collect(),
    )
}

#[cfg(target_os = "macos")]
fn stop_launchd_service(report: &mut CommandReport) -> Result<bool> {
    let plist_path = crate::moon::launchd::plist_path()?;
    if !plist_path.exists() {
        report.detail(format!(
            "launchd.bootout=skipped reason=plist_missing path={}",
            plist_path.display()
        ));
        return Ok(false);
    }

    let bootout_out = crate::moon::launchd::bootout_service()?;
    if bootout_out.status.success() {
        report.detail("launchd.bootout=ok".to_string());
        return Ok(true);
    }

    report.detail(format!(
        "launchd.bootout=ignored ({})",
        crate::moon::launchd::summarize_command_failure(&bootout_out)
    ));
    Ok(false)
}

#[cfg(not(target_os = "macos"))]
fn stop_launchd_service(_report: &mut CommandReport) -> Result<bool> {
    Ok(false)
}

pub fn run() -> Result<CommandReport> {
    let mut report = CommandReport::new("stop");
    let paths = resolve_paths()?;
    let lock_path = daemon_lock_path(&paths);
    report.detail(format!("daemon_lock={}", lock_path.display()));
    let mut stopped_any = stop_launchd_service(&mut report)?;

    let mut handled_pids = Vec::new();
    match read_daemon_lock_payload(&paths) {
        Ok(Some(payload)) => {
            let pid = payload.pid;
            report.detail(format!("daemon_pid={pid}"));
            handled_pids.push(pid);
            let command_line = process_command_line(pid)?;
            if stop_pid(pid, &command_line, &mut report)? {
                stopped_any = true;
            }
        }
        Ok(None) => {
            report.detail("moon watcher daemon lock payload missing or absent".to_string());
        }
        Err(err) => {
            report.issue(format!(
                "failed to read daemon lock {}: {err:#}",
                lock_path.display()
            ));
        }
    }

    let moon_home_marker = paths.moon_home.display().to_string();
    match list_candidate_processes(&moon_home_marker) {
        Ok(processes) => {
            for (pid, command_line) in processes {
                if handled_pids.contains(&pid) {
                    continue;
                }
                if stop_pid(pid, &command_line, &mut report)? {
                    stopped_any = true;
                }
            }
        }
        Err(err) => {
            report.detail(format!("process scan skipped: {err:#}"));
        }
    }

    cleanup_lock_file(&lock_path, &mut report);
    if !stopped_any && report.ok {
        report.detail(
            "moon watcher daemon already stopped (no launchd job, lock, or stray process found)"
                .to_string(),
        );
    }
    Ok(report)
}
