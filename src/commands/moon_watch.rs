use anyhow::Result;

use crate::commands::CommandReport;
use crate::moon::watcher;

#[derive(Debug, Clone, Default)]
pub struct MoonWatchOptions {
    pub once: bool,
    pub daemon: bool,
    pub dry_run: bool,
}

pub fn run(opts: &MoonWatchOptions) -> Result<CommandReport> {
    let mut report = CommandReport::new("watch");

    if opts.once && opts.daemon {
        report.issue("invalid flags: use only one of --once or --daemon");
        return Ok(report);
    }
    if opts.daemon && opts.dry_run {
        report.issue("invalid flags: --dry-run is only valid with --once");
        return Ok(report);
    }

    if opts.daemon
        && let Ok(exe) = std::env::current_exe()
    {
        let exe_str = exe.display().to_string();
        if exe_str.contains("target/debug")
            || exe_str.contains("target/release")
            || exe_str.contains("target\\debug")
            || exe_str.contains("target\\release")
        {
            report.issue(
                "CRITICAL: Running the background daemon via `cargo run` is disabled for stability.",
            );
            report.issue(
                "Cargo run holds file locks and causes severe CPU/IO spikes when the daemon restarts.",
            );
            report.issue("Please install the binary to your path first: `cargo install --path .`");
            report.issue("Then start the daemon using the compiled binary: `moon watch --daemon`");
            return Ok(report);
        }
    }

    if opts.daemon {
        report.detail("starting moon watcher in daemon mode");
        watcher::run_daemon()?;
        return Ok(report);
    }

    let cycle = if opts.dry_run {
        watcher::run_once_with_options(watcher::WatchRunOptions {
            force_distill_now: false,
            dry_run: opts.dry_run,
        })?
    } else {
        watcher::run_once()?
    };
    report.detail("moon watcher cycle completed");
    if opts.dry_run {
        report.detail("dry_run=true".to_string());
    }
    report.detail(format!("state_file={}", cycle.state_file));
    report.detail(format!(
        "heartbeat_epoch_secs={}",
        cycle.heartbeat_epoch_secs
    ));
    report.detail(format!("poll_interval_secs={}", cycle.poll_interval_secs));
    report.detail(format!(
        "distill.max_per_cycle={}",
        cycle.distill_max_per_cycle
    ));
    report.detail(format!(
        "pending_raw_sessions={}",
        cycle.pending_raw_sessions
    ));
    report.detail(format!("project.runs={}", cycle.projected_sessions));
    report.detail(format!("pending_mlib_docs={}", cycle.pending_mlib_docs));
    report.detail(format!("distill.runs={}", cycle.distill_runs));
    report.detail(format!(
        "pending_embed_collections={}",
        cycle.pending_embed_collections
    ));
    report.detail(format!("embed.runs={}", cycle.embed_runs));
    report.detail(format!(
        "hot_collection.lifecycle_mode={}",
        cycle.hot_collection_lifecycle_mode
    ));
    report.detail(format!(
        "hot_collection.lifecycle_command_mode={}",
        cycle.hot_collection_lifecycle_command_mode
    ));
    if let Some(summary) = cycle.embed_last_summary {
        report.detail(format!("embed.result={summary}"));
    }
    report.detail(format!("syns.due={}", cycle.syns_due));
    if let Some(distill) = cycle.distill {
        report.detail(format!("distill.provider={}", distill.provider));
        report.detail(format!("distill.summary_path={}", distill.summary_path));
    }
    if let Some(result) = cycle.syns_result {
        report.detail(format!("syns.result={result}"));
    }

    Ok(report)
}
