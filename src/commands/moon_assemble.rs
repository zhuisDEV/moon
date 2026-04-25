use anyhow::Result;

use crate::commands::CommandReport;
use crate::moon::assemble::{
    assemble_context_with_excerpt, embedding_index_anchor_from_state, output_path, resolve_input,
    write_assembly_output,
};
use crate::moon::audit;
use crate::moon::config::load_config;
use crate::moon::context_packet::{
    ContextPacketInput, build_context_packet_from_projection,
    output_path as context_packet_output_path, write_context_packet_output,
};
use crate::moon::distill::extract_projection_snapshot;
use crate::moon::paths::resolve_paths;
use crate::moon::state::{load, save};

#[derive(Debug, Clone, Default)]
pub struct MoonAssembleOptions {
    pub source_path: Option<String>,
    pub session_id: Option<String>,
    pub dry_run: bool,
    pub replay_has_compaction_summary: bool,
}

pub fn run(opts: &MoonAssembleOptions) -> Result<CommandReport> {
    let paths = resolve_paths()?;
    let mut state = load(&paths)?;
    let mut input = resolve_input(
        &paths,
        opts.source_path.as_deref(),
        opts.session_id.as_deref(),
    )?;
    input.embedding_index_anchor = Some(embedding_index_anchor_from_state(
        &paths,
        &state,
        &input.session_id,
    ));
    let raw_source_path = input
        .raw_source_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("assemble raw source path is not valid UTF-8"))?;
    let snapshot = extract_projection_snapshot(raw_source_path)?;
    let output = assemble_context_with_excerpt(&input, &snapshot.excerpt)?;
    let assembly_output_path = output_path(&paths, &output.session_id);

    let mut report = CommandReport::new("assemble");
    report.detail(format!("assemble.session_id={}", output.session_id));
    report.detail(format!(
        "assemble.raw_source_path={}",
        output.raw_source_path
    ));
    report.detail(format!(
        "assemble.cleanse_summary_path={}",
        output.cleanse_summary_path.as_deref().unwrap_or("none")
    ));
    report.detail(format!(
        "assemble.output_path={}",
        assembly_output_path.display()
    ));
    report.detail(format!(
        "assemble.content_chars={}",
        output.content.chars().count()
    ));
    report.detail(format!(
        "assemble.assembled_at_epoch_secs={}",
        output.assembled_at_epoch_secs
    ));
    report.detail("assemble.raw_parse_count=1".to_string());

    let cfg = load_config().unwrap_or_default();
    let context_packet = cfg
        .context_packet
        .enabled
        .then(|| {
            build_context_packet_from_projection(
                &paths,
                &state,
                &cfg.context_packet,
                &ContextPacketInput {
                    session_id: output.session_id.clone(),
                    raw_source_path: input.raw_source_path.clone(),
                    cleanse_summary_path: input.cleanse_summary_path.clone(),
                    replay_has_compaction_summary: opts.replay_has_compaction_summary,
                    residential_timezone: cfg.distill.residential_timezone.clone(),
                },
                &snapshot.data,
            )
        })
        .transpose()?;
    if let Some(packet) = context_packet.as_ref() {
        report.detail(format!(
            "assemble.packet_output_path={}",
            context_packet_output_path(&paths, &packet.session_id).display()
        ));
        report.detail(format!(
            "assemble.packet_chars={}",
            packet.content.chars().count()
        ));
        report.detail(format!(
            "assemble.packet_candidate_count={}",
            packet.candidate_count
        ));
        report.detail(format!(
            "assemble.packet_primary_source_family={}",
            packet.primary_source_family
        ));
        report.detail(format!(
            "assemble.packet_fallback_source={}",
            packet.fallback_source.as_deref().unwrap_or("none")
        ));
        report.detail(format!(
            "assemble.packet_source_reads={}",
            packet.source_read_count
        ));
        report.detail(format!(
            "assemble.packet_qmd_queries={}",
            packet.qmd_query_count
        ));
        report.detail(format!(
            "assemble.packet_coverage_decision={}",
            packet.coverage_decision
        ));
        report.detail(format!(
            "assemble.packet_coverage_reason={}",
            packet.coverage_reason
        ));
        report.detail(format!(
            "assemble.packet_positive_candidate_count={}",
            packet.positive_candidate_count
        ));
        report.detail(format!("assemble.packet_top_score={}", packet.top_score));
    }

    if opts.dry_run {
        report.detail("assemble.dry_run=true".to_string());
        return Ok(report);
    }

    let written_path = write_assembly_output(&paths, &output.session_id, &output.content)?;
    report.detail(format!("assemble.written_path={}", written_path.display()));
    if let Some(packet) = context_packet.as_ref() {
        let written_packet_path =
            write_context_packet_output(&paths, &packet.session_id, &packet.content)?;
        report.detail(format!(
            "assemble.packet_written_path={}",
            written_packet_path.display()
        ));
        state.last_context_packet_session_id = Some(packet.session_id.clone());
        state.last_context_packet_epoch_secs = Some(packet.packet_at_epoch_secs);
        state.last_context_packet_generation = Some(packet.generation.clone());
        state.last_context_packet_candidate_count = Some(packet.candidate_count);
    }

    state.last_session_id = Some(output.session_id.clone());
    state.last_assembly_session_id = Some(output.session_id.clone());
    state.last_assembly_epoch_secs = Some(output.assembled_at_epoch_secs);
    let state_file = save(&paths, &state)?;
    report.detail(format!("state_file={}", state_file.display()));

    let _ = audit::append_event(
        &paths,
        "assemble",
        "ok",
        &format!(
            "session_id={} raw={} cleanse={} output={}",
            output.session_id,
            output.raw_source_path,
            output.cleanse_summary_path.as_deref().unwrap_or("none"),
            written_path.display()
        ),
    );

    Ok(report)
}
