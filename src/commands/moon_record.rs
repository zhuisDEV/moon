use anyhow::Result;

use crate::commands::CommandReport;
use crate::moon::audit;
use crate::moon::paths::resolve_paths;
use crate::moon::record;
use crate::moon::state::{load, save};

#[derive(Debug, Clone, Default)]
pub struct MoonRecordOptions {
    pub source_path: Option<String>,
    pub session_id: Option<String>,
    pub dry_run: bool,
}

pub fn run(opts: &MoonRecordOptions) -> Result<CommandReport> {
    let paths = resolve_paths()?;
    let plan = record::plan_record(
        &paths,
        opts.source_path.as_deref(),
        opts.session_id.as_deref(),
    )?;

    let mut report = CommandReport::new("record");
    report.detail(format!("record.session_id={}", plan.session_id));
    report.detail(format!("record.source_path={}", plan.source_path.display()));
    report.detail(format!("record.source_selector={}", plan.selected_via));
    report.detail(format!("record.target_path={}", plan.target_path.display()));

    if opts.dry_run {
        report.detail("record.dry_run=true".to_string());
        return Ok(report);
    }

    let result = record::execute_record(&paths, &plan)?;
    report.detail(format!("record.copied_bytes={}", result.copied_bytes));

    let mut state = load(&paths)?;
    state.last_session_id = Some(plan.session_id.clone());
    let state_file = save(&paths, &state)?;
    report.detail(format!("state_file={}", state_file.display()));

    let _ = audit::append_event(
        &paths,
        "record",
        "ok",
        &format!(
            "session_id={} source={} target={}",
            plan.session_id,
            plan.source_path.display(),
            plan.target_path.display()
        ),
    );

    Ok(report)
}
