use anyhow::Result;

use crate::commands::CommandReport;
use crate::moon::context_engine::{CheckpointOptions, PressureSnapshot, run_checkpoint};
use crate::moon::paths::resolve_paths;

#[derive(Debug, Clone, Default)]
pub struct MoonContextEngineOptions {
    pub source_path: Option<String>,
    pub session_id: Option<String>,
    pub used_tokens: Option<u64>,
    pub max_tokens: Option<u64>,
    pub force_cleanse: bool,
}

pub fn run(opts: &MoonContextEngineOptions) -> Result<CommandReport> {
    let paths = resolve_paths()?;
    let mut report = CommandReport::new("context-engine");

    let pressure = match (opts.used_tokens, opts.max_tokens) {
        (None, None) => None,
        (Some(used_tokens), Some(max_tokens)) => Some(PressureSnapshot {
            used_tokens,
            max_tokens,
        }),
        _ => {
            report.issue(
                "context-engine requires both --used-tokens and --max-tokens when providing pressure"
                    .to_string(),
            );
            return Ok(report);
        }
    };

    let output = run_checkpoint(
        &paths,
        &CheckpointOptions {
            source_path: opts.source_path.clone(),
            session_id: opts.session_id.clone(),
            pressure,
            force_cleanse: opts.force_cleanse,
        },
    )?;

    report.detail(format!("context_engine.session_id={}", output.session_id));
    report.detail(format!(
        "context_engine.record_target_path={}",
        output.record_target_path
    ));
    report.detail(format!(
        "context_engine.cleanse_summary_path={}",
        output.cleanse_summary_path.as_deref().unwrap_or("none")
    ));
    report.detail(format!(
        "context_engine.cleanse_reason={}",
        output.cleanse_reason
    ));
    if let Some(embed_now) = output.embed_now.as_deref() {
        report.detail(format!("context_engine.embed_now={embed_now}"));
        if embed_now.starts_with("degraded ") {
            report.detail(format!("context_engine.embed_error={embed_now}"));
        }
    }
    report.detail(format!(
        "context_engine.assembly_path={}",
        output.assembly_output_path
    ));
    report.detail(format!(
        "context_engine.assembly_chars={}",
        output.assembly.content.chars().count()
    ));

    Ok(report)
}
