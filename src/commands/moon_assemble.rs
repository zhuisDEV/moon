use anyhow::Result;

use crate::commands::CommandReport;
use crate::moon::assemble::{
    assemble_context, embedding_index_anchor_from_state, output_path, resolve_input,
    write_assembly_output,
};
use crate::moon::audit;
use crate::moon::paths::resolve_paths;
use crate::moon::state::{load, save};

#[derive(Debug, Clone, Default)]
pub struct MoonAssembleOptions {
    pub source_path: Option<String>,
    pub session_id: Option<String>,
    pub dry_run: bool,
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
    let output = assemble_context(&input)?;
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

    if opts.dry_run {
        report.detail("assemble.dry_run=true".to_string());
        return Ok(report);
    }

    let written_path = write_assembly_output(&paths, &output.session_id, &output.content)?;
    report.detail(format!("assemble.written_path={}", written_path.display()));

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
