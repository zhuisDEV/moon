use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use crate::commands::CommandReport;
use crate::moon::audit;
use crate::moon::cleanse::{
    CleanseInput, render_summary_document, resolved_cleanse_model_label, run_cleanse,
};
use crate::moon::distill::load_source_excerpt;
use crate::moon::files::{ensure_readable_file, latest_file_with_extension, resolve_session_id};
use crate::moon::paths::{MoonPaths, resolve_paths};
use crate::moon::state::{load, save};

#[derive(Debug, Clone, Default)]
pub struct MoonCleanseOptions {
    pub source_path: Option<String>,
    pub session_id: Option<String>,
    pub dry_run: bool,
}

pub fn run(opts: &MoonCleanseOptions) -> Result<CommandReport> {
    let paths = resolve_paths()?;
    let source_path = select_source_path(&paths, opts)?;
    let session_id = resolve_session_id(opts.session_id.as_deref(), &source_path)?;
    let source_path_str = source_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("cleanse source path is not valid UTF-8"))?
        .to_string();
    let summary_path = paths.cleanse_dir.join(format!("{session_id}.md"));

    let mut report = CommandReport::new("cleanse");
    report.detail(format!("cleanse.session_id={session_id}"));
    report.detail(format!("cleanse.source_path={}", source_path.display()));
    report.detail(format!("cleanse.summary_path={}", summary_path.display()));
    report.detail(format!("cleanse.model={}", resolved_cleanse_model_label()));

    if opts.dry_run {
        report.detail("cleanse.dry_run=true".to_string());
        return Ok(report);
    }

    let source_excerpt = load_source_excerpt(&source_path_str).with_context(|| {
        format!(
            "failed to derive cleanse input from {}",
            source_path.display()
        )
    })?;
    let output = run_cleanse(&CleanseInput {
        session_id: session_id.clone(),
        source_path: source_path_str.clone(),
        source_excerpt,
    })?;

    fs::create_dir_all(&paths.cleanse_dir)
        .with_context(|| format!("failed to create {}", paths.cleanse_dir.display()))?;
    let rendered = render_summary_document(
        &session_id,
        &source_path_str,
        &output.provider,
        &output.model,
        output.created_at_epoch_secs,
        &output.summary,
    );
    fs::write(&summary_path, rendered.as_bytes())
        .with_context(|| format!("failed to write {}", summary_path.display()))?;
    let written_bytes = fs::metadata(&summary_path)
        .with_context(|| format!("failed to stat {}", summary_path.display()))?
        .len();

    let mut state = load(&paths)?;
    state.last_session_id = Some(session_id.clone());
    state.last_compaction_trigger_epoch_secs = Some(output.created_at_epoch_secs);
    state.last_provider = Some(output.provider.clone());
    let state_file = save(&paths, &state)?;

    report.detail(format!("cleanse.provider={}", output.provider));
    report.detail(format!(
        "cleanse.summary_chars={}",
        output.summary.chars().count()
    ));
    report.detail(format!("cleanse.written_bytes={written_bytes}"));
    report.detail("cleanse.pending_projection=false".to_string());
    report.detail("cleanse.pending_embed=false".to_string());
    report.detail(format!("state_file={}", state_file.display()));

    let _ = audit::append_event(
        &paths,
        "cleanse",
        "ok",
        &format!(
            "session_id={} source={} summary={} provider={} model={}",
            session_id,
            source_path.display(),
            summary_path.display(),
            output.provider,
            output.model
        ),
    );

    Ok(report)
}

fn select_source_path(paths: &MoonPaths, opts: &MoonCleanseOptions) -> Result<PathBuf> {
    if let Some(source_path) = opts.source_path.as_deref()
        && !source_path.trim().is_empty()
    {
        let path = PathBuf::from(source_path.trim());
        ensure_readable_file(&path, "cleanse")?;
        return Ok(path);
    }

    if let Some(session_id) = opts.session_id.as_deref()
        && !session_id.trim().is_empty()
    {
        let raw_path = paths.raw_dir.join(format!("{}.jsonl", session_id.trim()));
        if raw_path.is_file() {
            return Ok(raw_path);
        }

        anyhow::bail!(
            "cleanse source not found for session `{}` under {}",
            session_id.trim(),
            paths.raw_dir.display()
        );
    }

    latest_raw_file(&paths.raw_dir)?
        .ok_or_else(|| anyhow::anyhow!("no raw sources found under {}", paths.raw_dir.display()))
}

fn latest_raw_file(raw_dir: &Path) -> Result<Option<PathBuf>> {
    latest_file_with_extension(raw_dir, "jsonl", false)
}
