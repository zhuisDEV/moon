use anyhow::{Context, Result};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread;
use std::time::Duration;

const DOCTOR_TIMEOUT_SECS: u64 = 30;

fn ensure_executable_path(path: &Path) -> Result<()> {
    let meta = fs::metadata(path)
        .with_context(|| format!("openclaw binary path does not exist: {}", path.display()))?;
    if !meta.is_file() {
        anyhow::bail!("openclaw binary path is not a file: {}", path.display());
    }
    Ok(())
}

pub(crate) fn resolve_openclaw_bin_path() -> Result<PathBuf> {
    match env::var("OPENCLAW_BIN") {
        Ok(custom) => {
            let trimmed = custom.trim();
            if trimmed.is_empty() {
                anyhow::bail!("OPENCLAW_BIN is set but empty");
            }
            let path = PathBuf::from(trimmed);
            ensure_executable_path(&path)?;
            return Ok(path);
        }
        Err(env::VarError::NotUnicode(_)) => {
            anyhow::bail!("OPENCLAW_BIN contains invalid unicode");
        }
        Err(env::VarError::NotPresent) => {}
    }

    let resolved = which::which("openclaw")
        .context("openclaw binary not found; set OPENCLAW_BIN or add openclaw to PATH")?;
    ensure_executable_path(&resolved)?;
    Ok(resolved)
}

fn run_openclaw(args: &[&str]) -> Result<Output> {
    run_openclaw_with_optional_timeout(args, None)
}

fn run_openclaw_with_optional_timeout(args: &[&str], timeout_secs: Option<u64>) -> Result<Output> {
    let bin = resolve_openclaw_bin_path()?;
    let mut cmd = Command::new(&bin);
    cmd.args(args);
    let out = match timeout_secs {
        Some(timeout_secs) => {
            crate::moon::util::run_command_with_optional_timeout(&mut cmd, Some(timeout_secs))
        }
        None => crate::moon::util::run_command_with_timeout(&mut cmd),
    }
    .with_context(|| format!("failed to run `{}` {}", bin.display(), args.join(" ")))?;
    Ok(out)
}

pub fn run_openclaw_retry(args: &[&str], retries: usize) -> Result<Output> {
    run_openclaw_retry_with_optional_timeout(args, retries, None)
}

fn run_openclaw_retry_with_optional_timeout(
    args: &[&str],
    retries: usize,
    timeout_secs: Option<u64>,
) -> Result<Output> {
    let mut last_out: Option<Output> = None;

    for attempt in 0..=retries {
        let out = run_openclaw_with_optional_timeout(args, timeout_secs)?;
        if out.status.success() {
            return Ok(out);
        }
        last_out = Some(out);
        if attempt < retries {
            let delay_ms = 250 * (attempt + 1) as u64;
            thread::sleep(Duration::from_millis(delay_ms));
        }
    }

    let Some(out) = last_out else {
        anyhow::bail!(
            "command failed after retries without output: openclaw {}",
            args.join(" ")
        );
    };
    anyhow::bail!(
        "command failed after retries: openclaw {}\nstdout: {}\nstderr: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

pub fn try_plugins_install(path: &Path) -> Result<()> {
    let path_str = path.to_string_lossy().to_string();
    let out = run_openclaw(&["plugins", "install", &path_str]);

    match out {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr).to_string();
            if stderr.contains("plugin already exists") || stderr.contains("already exists") {
                return Ok(());
            }
            anyhow::bail!("openclaw plugins install failed: {}", stderr.trim())
        }
        Err(err) => Err(err),
    }
}

pub fn try_plugins_uninstall(plugin_id: &str) -> Result<()> {
    let out = run_openclaw(&["plugins", "uninstall", plugin_id]);

    match out {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr).to_string();
            let lower = stderr.to_ascii_lowercase();
            if lower.contains("not installed")
                || lower.contains("not found")
                || lower.contains("unknown plugin")
            {
                return Ok(());
            }
            anyhow::bail!("openclaw plugins uninstall failed: {}", stderr.trim())
        }
        Err(err) => Err(err),
    }
}

pub fn run_gateway_restart(retries: usize) -> Result<()> {
    run_openclaw_retry(&["gateway", "restart"], retries)?;
    Ok(())
}

pub fn run_gateway_stop_start() -> Result<()> {
    run_openclaw_retry(&["gateway", "stop"], 1)?;
    run_openclaw_retry(&["gateway", "start"], 1)?;
    Ok(())
}

pub fn run_doctor() -> Result<()> {
    run_openclaw_retry_with_optional_timeout(
        &["doctor", "--non-interactive"],
        2,
        Some(DOCTOR_TIMEOUT_SECS),
    )?;
    Ok(())
}

pub fn plugins_list_json() -> Result<String> {
    let out = run_openclaw_retry(&["plugins", "list", "--json"], 1)?;
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

pub fn plugins_info_json(plugin_id: &str) -> Result<String> {
    let out = run_openclaw_retry(&["plugins", "info", plugin_id, "--json"], 1)?;
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

pub fn openclaw_available() -> bool {
    resolve_openclaw_bin_path().is_ok()
}
