use anyhow::Result;

use crate::commands::CommandReport;
use crate::moon::audit;
use crate::moon::paths::resolve_paths;
use crate::moon::project::{ProjectLane, ProjectRunOptions, run_and_mark_embed_pending};
use crate::moon::state::{load, save};

#[derive(Debug, Clone, Default)]
pub struct MoonProjectOptions {
    pub source_path: Option<String>,
    pub session_id: Option<String>,
    pub lane: ProjectLane,
    pub dry_run: bool,
}

pub fn run(opts: &MoonProjectOptions) -> Result<CommandReport> {
    let paths = resolve_paths()?;
    let mut state = load(&paths)?;
    let output = run_and_mark_embed_pending(
        &paths,
        &mut state,
        &ProjectRunOptions {
            source_path: opts.source_path.clone(),
            session_id: opts.session_id.clone(),
            lane: opts.lane,
            dry_run: opts.dry_run,
        },
    )?;

    let mut report = CommandReport::new("project");
    report.detail(format!("project.session_id={}", output.session_id));
    report.detail(format!(
        "project.source_path={}",
        output.source_path.display()
    ));
    report.detail(format!(
        "project.target_path={}",
        output.target_path.display()
    ));
    report.detail(format!("project.lane={}", output.lane.as_str()));
    report.detail(format!(
        "project.embed_collection={}",
        output.embed_collection
    ));
    report.detail(format!("project.message_count={}", output.message_count));
    report.detail(format!(
        "project.filtered_noise_count={}",
        output.filtered_noise_count
    ));
    report.detail(format!(
        "project.tool_call_count={}",
        output.tool_call_count
    ));
    report.detail(format!("project.truncated={}", output.truncated));

    if opts.dry_run {
        report.detail("project.dry_run=true".to_string());
        return Ok(report);
    }

    if let Some(written_bytes) = output.written_bytes {
        report.detail(format!("project.written_bytes={written_bytes}"));
    }

    state.last_session_id = Some(output.session_id.clone());
    let state_file = save(&paths, &state)?;
    report.detail("project.pending_embed=true".to_string());
    report.detail(format!("state_file={}", state_file.display()));

    let _ = audit::append_event(
        &paths,
        "project",
        "ok",
        &format!(
            "session_id={} source={} target={} messages={} filtered_noise={}",
            output.session_id,
            output.source_path.display(),
            output.target_path.display(),
            output.message_count,
            output.filtered_noise_count
        ),
    );

    Ok(report)
}
