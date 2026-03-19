#[cfg(target_os = "macos")]
use anyhow::{Context, Result};
#[cfg(target_os = "macos")]
use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::process::{Command, Output};

#[cfg(target_os = "macos")]
pub const LAUNCHD_LABEL: &str = "com.moon.watch";
#[cfg(target_os = "macos")]
pub const LAUNCHD_PLIST_NAME: &str = "com.moon.watch.plist";

#[cfg(target_os = "macos")]
pub fn plist_path() -> Result<PathBuf> {
    let home_dir = dirs::home_dir().context("HOME directory could not be resolved")?;
    Ok(home_dir
        .join("Library")
        .join("LaunchAgents")
        .join(LAUNCHD_PLIST_NAME))
}

#[cfg(target_os = "macos")]
fn resolve_uid() -> Result<String> {
    let output = Command::new("id")
        .arg("-u")
        .output()
        .context("failed to resolve user id via `id -u`")?;
    if !output.status.success() {
        anyhow::bail!("`id -u` failed: {}", summarize_command_failure(&output));
    }

    let uid = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if uid.is_empty() {
        anyhow::bail!("`id -u` returned empty output");
    }
    Ok(uid)
}

#[cfg(target_os = "macos")]
fn launchd_domain() -> Result<String> {
    Ok(format!("gui/{}", resolve_uid()?))
}

#[cfg(target_os = "macos")]
fn run_launchctl(args: &[&str]) -> Result<Output> {
    Command::new("launchctl")
        .args(args)
        .output()
        .with_context(|| format!("failed to execute launchctl {}", args.join(" ")))
}

#[cfg(target_os = "macos")]
pub fn bootout_service() -> Result<Output> {
    let domain = launchd_domain()?;
    let plist_path = plist_path()?;
    let plist_arg = plist_path.display().to_string();
    run_launchctl(["bootout", &domain, &plist_arg].as_slice())
}

#[cfg(target_os = "macos")]
pub fn bootstrap_service() -> Result<Output> {
    let domain = launchd_domain()?;
    let plist_path = plist_path()?;
    let plist_arg = plist_path.display().to_string();
    run_launchctl(["bootstrap", &domain, &plist_arg].as_slice())
}

#[cfg(target_os = "macos")]
pub fn kickstart_service() -> Result<Output> {
    let domain = launchd_domain()?;
    let target = format!("{domain}/{LAUNCHD_LABEL}");
    run_launchctl(["kickstart", "-k", &target].as_slice())
}

#[cfg(target_os = "macos")]
pub fn summarize_command_failure(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stderr.is_empty() {
        return stderr;
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !stdout.is_empty() {
        return stdout;
    }
    match output.status.code() {
        Some(code) => format!("exit code {code}"),
        None => "terminated by signal".to_string(),
    }
}
