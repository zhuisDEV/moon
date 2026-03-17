use anyhow::Result;

use crate::commands::CommandReport;
use crate::moon::audit;
use crate::moon::config::load_config;
use crate::moon::embed::{self, EmbedCaller, EmbedRunError, EmbedRunOptions};
use crate::moon::paths::resolve_paths;
use crate::moon::state;

#[derive(Debug, Clone)]
pub struct MoonEmbedOptions {
    pub collection_name: String,
    pub max_docs: usize,
    pub dry_run: bool,
    pub watcher_trigger: bool,
}

pub fn run(opts: &MoonEmbedOptions) -> Result<CommandReport> {
    let paths = resolve_paths()?;
    crate::moon::qmd::install_runtime_env(&paths);
    let cfg = load_config()?;
    let mut state = state::load(&paths)?;
    let mut report = CommandReport::new("embed");

    let caller = if opts.watcher_trigger {
        EmbedCaller::Watcher
    } else {
        EmbedCaller::Manual
    };
    let run_opts = EmbedRunOptions {
        collection_name: opts.collection_name.clone(),
        max_docs: opts.max_docs,
        dry_run: opts.dry_run,
        caller,
        max_cycle_secs: Some(300), // Default 300s for manual/command-line runs
    };

    let run_result = embed::run(&paths, &mut state, &cfg.embed, &run_opts);
    let state_file = state::save(&paths, &state)?;
    report.detail(format!("state_file={}", state_file.display()));

    match run_result {
        Ok(summary) => {
            report.detail(format!("collection={}", summary.collection));
            report.detail(format!("embed.mode={}", summary.mode));
            report.detail(format!("embed.capability={}", summary.capability));
            report.detail(format!(
                "embed.requested_max_docs={}",
                summary.requested_max_docs
            ));
            report.detail(format!("embed.selected_docs={}", summary.selected_docs));
            report.detail(format!("embed.embedded_docs={}", summary.embedded_docs));
            report.detail(format!("embed.pending_before={}", summary.pending_before));
            report.detail(format!("embed.pending_after={}", summary.pending_after));
            report.detail(format!("embed.elapsed_ms={}", summary.elapsed_ms));
            report.detail(format!("embed.degraded={}", summary.degraded));
            report.detail(format!("embed.skip_reason={}", summary.skip_reason));

            let status = if summary.degraded { "degraded" } else { "ok" };
            let _ = audit::append_event(
                &paths,
                "embed",
                status,
                &format!(
                    "mode={} collection={} capability={} selected={} embedded={} pending_before={} pending_after={} skip_reason={}",
                    summary.mode,
                    summary.collection,
                    summary.capability,
                    summary.selected_docs,
                    summary.embedded_docs,
                    summary.pending_before,
                    summary.pending_after,
                    summary.skip_reason
                ),
            );
        }
        Err(err) => {
            let err_text = format!("{err}");
            let status = match &err {
                EmbedRunError::CapabilityMissing(_) | EmbedRunError::Locked(_) => "degraded",
                EmbedRunError::StatusFailed(_) | EmbedRunError::Failed(_) => "failed",
            };
            let _ = audit::append_event(
                &paths,
                "embed",
                status,
                &format!(
                    "mode={} collection={} error={err_text}",
                    caller.as_str(),
                    opts.collection_name
                ),
            );

            if opts.watcher_trigger
                && matches!(
                    err,
                    EmbedRunError::CapabilityMissing(_) | EmbedRunError::Locked(_)
                )
            {
                report.detail(format!("embed.degraded=true error={err_text}"));
            } else {
                report.issue(err_text);
            }
        }
    }

    Ok(report)
}
