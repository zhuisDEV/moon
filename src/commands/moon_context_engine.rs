use anyhow::Result;

use crate::commands::CommandReport;
use crate::moon::context_engine::{
    CheckpointOptions, PressureSnapshot, run_checkpoint, run_sync_checkpoint,
};
use crate::moon::paths::resolve_paths;

#[derive(Debug, Clone, Default)]
pub struct MoonContextEngineOptions {
    pub source_path: Option<String>,
    pub session_id: Option<String>,
    pub used_tokens: Option<u64>,
    pub max_tokens: Option<u64>,
    pub force_cleanse: bool,
    pub replay_has_compaction_summary: bool,
    pub sync_only: bool,
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

    let checkpoint_opts = CheckpointOptions {
        source_path: opts.source_path.clone(),
        session_id: opts.session_id.clone(),
        pressure,
        force_cleanse: opts.force_cleanse,
        replay_has_compaction_summary: opts.replay_has_compaction_summary,
    };

    if opts.sync_only {
        let output = run_sync_checkpoint(&paths, &checkpoint_opts)?;
        report.detail("context_engine.sync_only=true".to_string());
        report.detail(format!("context_engine.session_id={}", output.session_id));
        report.detail(format!(
            "context_engine.record_target_path={}",
            output.record_target_path
        ));
        report.detail(format!(
            "context_engine.project_path={}",
            output.project_output_path
        ));
        report.detail(format!(
            "context_engine.project_status={}",
            output.project_status
        ));
        report.detail(format!("context_engine.sync_reason={}", output.sync_reason));
        return Ok(report);
    }

    let output = run_checkpoint(&paths, &checkpoint_opts)?;

    report.detail(format!("context_engine.session_id={}", output.session_id));
    report.detail(format!(
        "context_engine.record_target_path={}",
        output.record_target_path
    ));
    report.detail(format!(
        "context_engine.project_path={}",
        output.project_output_path
    ));
    report.detail(format!(
        "context_engine.project_status={}",
        output.project_status
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
    report.detail(format!(
        "context_engine.packet_path={}",
        output
            .context_packet_output_path
            .as_deref()
            .unwrap_or("none")
    ));
    report.detail(format!(
        "context_engine.packet_chars={}",
        output
            .context_packet
            .as_ref()
            .map(|packet| packet.content.chars().count())
            .unwrap_or(0)
    ));
    report.detail(format!(
        "context_engine.packet_candidate_count={}",
        output
            .context_packet
            .as_ref()
            .map(|packet| packet.candidate_count)
            .unwrap_or(0)
    ));
    report.detail(format!(
        "context_engine.packet_cache_hit={}",
        output
            .context_packet
            .as_ref()
            .map(|packet| packet.cache_hit)
            .unwrap_or(false)
    ));
    report.detail(format!(
        "context_engine.packet_query={}",
        output
            .context_packet
            .as_ref()
            .map(|packet| packet.query.as_str())
            .unwrap_or("none")
    ));
    report.detail(format!(
        "context_engine.packet_primary_source_family={}",
        output
            .context_packet
            .as_ref()
            .map(|packet| packet.primary_source_family.as_str())
            .unwrap_or("none")
    ));
    report.detail(format!(
        "context_engine.packet_fallback_source={}",
        output
            .context_packet
            .as_ref()
            .and_then(|packet| packet.fallback_source.as_deref())
            .unwrap_or("none")
    ));
    report.detail(format!(
        "context_engine.packet_source_reads={}",
        output
            .context_packet
            .as_ref()
            .map(|packet| packet.source_read_count)
            .unwrap_or(0)
    ));
    report.detail(format!(
        "context_engine.packet_qmd_queries={}",
        output
            .context_packet
            .as_ref()
            .map(|packet| packet.qmd_query_count)
            .unwrap_or(0)
    ));
    report.detail(format!(
        "context_engine.packet_coverage_decision={}",
        output
            .context_packet
            .as_ref()
            .map(|packet| packet.coverage_decision.as_str())
            .unwrap_or("none")
    ));
    report.detail(format!(
        "context_engine.packet_coverage_reason={}",
        output
            .context_packet
            .as_ref()
            .map(|packet| packet.coverage_reason.as_str())
            .unwrap_or("none")
    ));
    report.detail(format!(
        "context_engine.packet_positive_candidate_count={}",
        output
            .context_packet
            .as_ref()
            .map(|packet| packet.positive_candidate_count)
            .unwrap_or(0)
    ));
    report.detail(format!(
        "context_engine.packet_top_score={}",
        output
            .context_packet
            .as_ref()
            .map(|packet| packet.top_score)
            .unwrap_or(0)
    ));
    report.detail(format!(
        "context_engine.raw_parse_count={}",
        output.raw_parse_count
    ));

    Ok(report)
}
