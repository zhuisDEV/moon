use anyhow::{Context, Result};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

use crate::moon::files::{ensure_readable_file, latest_file_with_extension, resolve_session_id};
use crate::moon::paths::MoonPaths;

#[derive(Debug, Clone)]
pub struct RecordPlan {
    pub session_id: String,
    pub source_path: PathBuf,
    pub target_path: PathBuf,
    pub selected_via: &'static str,
}

#[derive(Debug, Clone)]
pub struct RecordResult {
    pub copied_bytes: u64,
}

#[derive(Debug, Clone)]
struct SelectedSource {
    session_id: String,
    path: PathBuf,
    selected_via: &'static str,
}

pub fn plan_record(
    paths: &MoonPaths,
    source_path: Option<&str>,
    session_id: Option<&str>,
) -> Result<RecordPlan> {
    let selected = select_source(paths, source_path, session_id)?;
    let target_path = paths.raw_dir.join(format!("{}.jsonl", selected.session_id));

    Ok(RecordPlan {
        session_id: selected.session_id,
        source_path: selected.path,
        target_path,
        selected_via: selected.selected_via,
    })
}

pub fn execute_record(paths: &MoonPaths, plan: &RecordPlan) -> Result<RecordResult> {
    fs::create_dir_all(&paths.raw_dir)
        .with_context(|| format!("failed to create {}", paths.raw_dir.display()))?;
    fs::copy(&plan.source_path, &plan.target_path).with_context(|| {
        format!(
            "failed to copy raw session from {} to {}",
            plan.source_path.display(),
            plan.target_path.display()
        )
    })?;

    let copied_bytes = fs::metadata(&plan.target_path)
        .with_context(|| format!("failed to stat {}", plan.target_path.display()))?
        .len();

    Ok(RecordResult { copied_bytes })
}

fn select_source(
    paths: &MoonPaths,
    source_path: Option<&str>,
    session_id: Option<&str>,
) -> Result<SelectedSource> {
    if let Some(source_path) = source_path
        && !source_path.trim().is_empty()
    {
        let path = PathBuf::from(source_path.trim());
        ensure_readable_file(&path, "record")?;
        let session_id = resolve_session_id(session_id, &path)?;
        return Ok(SelectedSource {
            session_id,
            path,
            selected_via: "explicit-source",
        });
    }

    if let Some(session_id) = session_id
        && !session_id.trim().is_empty()
    {
        let path = paths
            .openclaw_sessions_dir
            .join(format!("{}.jsonl", session_id.trim()));
        ensure_readable_file(&path, "record")?;
        return Ok(SelectedSource {
            session_id: session_id.trim().to_string(),
            path,
            selected_via: "explicit-session-id",
        });
    }

    if let Some(selected) = select_from_manifest(&paths.openclaw_sessions_dir)? {
        return Ok(selected);
    }

    if let Some(path) = latest_session_file(&paths.openclaw_sessions_dir)? {
        let session_id = resolve_session_id(None, &path)?;
        return Ok(SelectedSource {
            session_id,
            path,
            selected_via: "latest-session-file",
        });
    }

    anyhow::bail!(
        "no OpenClaw session sources found under {}",
        paths.openclaw_sessions_dir.display()
    );
}

fn select_from_manifest(sessions_dir: &Path) -> Result<Option<SelectedSource>> {
    let manifest_path = sessions_dir.join("sessions.json");
    if !manifest_path.is_file() {
        return Ok(None);
    }

    let raw = fs::read_to_string(&manifest_path)
        .with_context(|| format!("failed to read {}", manifest_path.display()))?;
    let value: Value = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse {}", manifest_path.display()))?;
    let Some(entries) = value.as_object() else {
        return Ok(None);
    };

    let selected = entries
        .values()
        .filter_map(|entry| {
            let session_id = entry.get("sessionId")?.as_str()?.trim();
            if session_id.is_empty() {
                return None;
            }
            let path = sessions_dir.join(format!("{session_id}.jsonl"));
            if !path.is_file() {
                return None;
            }
            let updated_at = entry.get("updatedAt").and_then(Value::as_u64).unwrap_or(0);
            Some((updated_at, session_id.to_string(), path))
        })
        .max_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.cmp(&right.1))
                .then_with(|| left.2.cmp(&right.2))
        });

    Ok(selected.map(|(_, session_id, path)| SelectedSource {
        session_id,
        path,
        selected_via: "sessions-manifest",
    }))
}

fn latest_session_file(sessions_dir: &Path) -> Result<Option<PathBuf>> {
    latest_file_with_extension(sessions_dir, "jsonl", true)
}
