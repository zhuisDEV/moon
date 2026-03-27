use anyhow::Result;

use crate::commands::install::{self, InstallOptions};
use crate::commands::verify::{self, VerifyOptions};
use crate::commands::{CommandReport, ensure_openclaw_available, restart_gateway_with_fallback};

#[derive(Debug, Clone, Default)]
pub struct RepairOptions {
    pub force: bool,
}

pub fn run(opts: &RepairOptions) -> Result<CommandReport> {
    let mut report = CommandReport::new("repair");
    if opts.force {
        report.detail("force mode requested".to_string());
    }

    if !ensure_openclaw_available(&mut report) {
        return Ok(report);
    }

    report.merge(install::run(&InstallOptions {
        force: true,
        dry_run: false,
        apply: true,
    })?);
    restart_gateway_with_fallback(&mut report);
    report.merge(verify::run(&VerifyOptions {
        strict: true,
        verbose: false,
    })?);

    Ok(report)
}
