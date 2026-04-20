use anyhow::Result;

use crate::commands::status;
use crate::commands::{CommandReport, ensure_openclaw_available};
use crate::openclaw::doctor;

#[derive(Debug, Clone, Default)]
pub struct VerifyOptions {
    pub strict: bool,
    pub verbose: bool,
}

fn detail_value(details: &[String], prefix: &str) -> Option<String> {
    details
        .iter()
        .find(|line| line.starts_with(prefix))
        .cloned()
}

pub fn run(opts: &VerifyOptions) -> Result<CommandReport> {
    let mut report = CommandReport::new("verify");
    report.detail("runtime.controller=moon-context-engine".to_string());
    report.detail("runtime.watcher_role=transitional-shell".to_string());

    let openclaw_ready = ensure_openclaw_available(&mut report);
    if openclaw_ready {
        if let Err(err) = doctor::run_full_doctor() {
            report.issue(format!("doctor failed: {err:#}"));
        } else {
            report.detail("doctor: ok".to_string());
        }
    }

    let status_report = status::run()?;
    if opts.verbose {
        report.merge(status_report);
    } else {
        report.detail(format!(
            "status.summary={} issues={}",
            if status_report.ok { "ok" } else { "failed" },
            status_report.issues.len()
        ));
        let mut surfaced_details = 0usize;
        for prefix in [
            "state_dir=",
            "config_path=",
            "plugin_dir=",
            "plugin_listed_by_openclaw=",
            "plugin_loaded_by_openclaw=",
            "plugin_assets_match_local=",
            "provenance repair hint:",
        ] {
            if let Some(detail) = detail_value(&status_report.details, prefix) {
                report.detail(detail);
                surfaced_details += 1;
            }
        }
        let suppressed_details = status_report.details.len().saturating_sub(surfaced_details);
        report.detail(format!(
            "status.details_suppressed={} (rerun with `moon verify --strict --verbose` for full details)",
            suppressed_details
        ));
        for issue in status_report.issues {
            report.issue(issue);
        }
    }

    if opts.strict && !report.ok {
        report.issue("strict verify failed");
    }

    Ok(report)
}
