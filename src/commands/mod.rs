pub mod install;
pub mod moon_assemble;
pub mod moon_cleanse;
pub mod moon_config;
pub mod moon_context_engine;
pub mod moon_distill;
pub mod moon_embed;
pub mod moon_health;
pub mod moon_project;
pub mod moon_recall;
pub mod moon_record;
pub mod moon_restart;
pub mod moon_status;
pub mod moon_stop;
pub mod moon_watch;
pub mod repair;
pub mod status;
pub mod verify;

use anyhow::{Context, Result};
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize)]
pub struct CommandReport {
    pub command: String,
    pub ok: bool,
    pub details: Vec<String>,
    pub issues: Vec<String>,
}

impl CommandReport {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            ok: true,
            details: Vec::new(),
            issues: Vec::new(),
        }
    }

    pub fn detail(&mut self, text: impl Into<String>) {
        self.details.push(text.into());
    }

    pub fn issue(&mut self, text: impl Into<String>) {
        self.ok = false;
        self.issues.push(text.into());
    }

    pub fn merge(&mut self, mut other: CommandReport) {
        self.ok &= other.ok;
        self.details.append(&mut other.details);
        self.issues.append(&mut other.issues);
    }
}

pub fn ensure_openclaw_available(report: &mut CommandReport) -> bool {
    if crate::openclaw::gateway::openclaw_available() {
        return true;
    }

    report.issue("openclaw binary unavailable; set OPENCLAW_BIN or ensure openclaw is on PATH");
    false
}

pub fn restart_gateway_with_fallback(report: &mut CommandReport) {
    if let Err(err) = crate::openclaw::gateway::run_gateway_restart(2) {
        report.issue(format!("gateway restart failed: {err}"));
        if let Err(fallback_err) = crate::openclaw::gateway::run_gateway_stop_start() {
            report.issue(format!(
                "gateway stop/start fallback failed: {fallback_err}"
            ));
        } else {
            report.detail("gateway stop/start fallback succeeded");
        }
    } else {
        report.detail("gateway restart succeeded");
    }
}
fn canonicalize_or_original(path: PathBuf) -> PathBuf {
    std::fs::canonicalize(&path).unwrap_or(path)
}

fn expected_workspace_from_lock(paths: &crate::moon::paths::MoonPaths) -> Option<PathBuf> {
    let payload = crate::moon::daemon_lock::read_daemon_lock_payload(paths)
        .ok()
        .flatten()?;
    if payload.moon_home.trim().is_empty() {
        return None;
    }
    Some(PathBuf::from(payload.moon_home.trim()))
}

pub fn validate_cwd(
    paths: &crate::moon::paths::MoonPaths,
    allow_out_of_bounds: bool,
) -> Result<()> {
    if allow_out_of_bounds {
        return Ok(());
    }

    // Prefer daemon-recorded workspace when available; fallback to explicit MOON_HOME only.
    let expected_workspace = expected_workspace_from_lock(paths)
        .or_else(|| paths.moon_home_is_explicit.then(|| paths.moon_home.clone()));

    let Some(expected_workspace) = expected_workspace else {
        return Ok(());
    };

    let cwd = std::env::current_dir().context("failed to resolve current working directory")?;
    let canon_cwd = canonicalize_or_original(cwd.clone());
    let canon_expected = canonicalize_or_original(expected_workspace.clone());

    // Allow parent/child paths so repo-root and MOON_HOME subdir flows both work.
    let in_bounds =
        canon_cwd.starts_with(&canon_expected) || canon_expected.starts_with(&canon_cwd);
    if in_bounds {
        return Ok(());
    }

    anyhow::bail!(
        "code={} cwd={} expected_workspace={} hint=run from the workspace tree or pass --allow-out-of-bounds",
        crate::error::MoonErrorCode::E004CwdInvalid.as_str(),
        cwd.display(),
        expected_workspace.display()
    );
}
