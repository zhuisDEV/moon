use anyhow::{Context, Result};
use chrono::{SecondsFormat, TimeZone, Utc};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::moon::distill::{ProjectionData, ProjectionEntry, extract_projection_data};
use crate::moon::files::{
    ensure_readable_file, gather_files_with_extension, latest_file_with_extension,
    resolve_session_id,
};
use crate::moon::paths::MoonPaths;
use crate::moon::state::{
    HOT_EMBED_COLLECTION_FALLBACK, LIBRARY_EMBED_COLLECTION, MoonState,
    hot_embed_collection_for_session, hot_projection_path_for_session, is_hot_embed_collection,
    mark_embed_maintenance_pending, remove_embedded_projection_collection,
    remove_embedded_projection_paths,
};
use crate::moon::util::now_epoch_secs;
use crate::moon::util::truncate_with_ellipsis;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ProjectLane {
    #[default]
    Hot,
    Library,
}

impl ProjectLane {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hot => "hot",
            Self::Library => "library",
        }
    }

    fn embed_collection(self, session_id: &str) -> String {
        match self {
            Self::Hot => hot_embed_collection_for_session(session_id),
            Self::Library => LIBRARY_EMBED_COLLECTION.to_string(),
        }
    }

    fn target_path(self, paths: &MoonPaths, session_id: &str) -> PathBuf {
        match self {
            Self::Hot => hot_projection_path_for_session(paths, session_id),
            Self::Library => paths.mlib_dir.join(format!("{session_id}.md")),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ProjectRunOptions {
    pub source_path: Option<String>,
    pub session_id: Option<String>,
    pub lane: ProjectLane,
    pub dry_run: bool,
}

#[derive(Debug, Clone)]
pub struct ProjectRunOutput {
    pub session_id: String,
    pub source_path: PathBuf,
    pub target_path: PathBuf,
    pub lane: ProjectLane,
    pub embed_collection: String,
    pub message_count: usize,
    pub filtered_noise_count: usize,
    pub tool_call_count: usize,
    pub truncated: bool,
    pub written_bytes: Option<u64>,
}

pub fn run(paths: &MoonPaths, opts: &ProjectRunOptions) -> Result<ProjectRunOutput> {
    let source_path = select_source_path(paths, opts)?;
    let session_id = resolve_session_id(opts.session_id.as_deref(), &source_path)?;
    let projection = extract_projection_data(
        source_path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("raw source path is not valid UTF-8"))?,
    )
    .with_context(|| format!("failed to parse raw source {}", source_path.display()))?;
    let target_path = opts.lane.target_path(paths, &session_id);
    let embed_collection = opts.lane.embed_collection(&session_id);

    let written_bytes = if opts.dry_run {
        None
    } else {
        let target_root = target_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("projection target has no parent directory"))?;
        fs::create_dir_all(target_root)
            .with_context(|| format!("failed to create {}", target_root.display()))?;
        let markdown = render_projection_markdown(&session_id, &source_path, &projection);
        fs::write(&target_path, markdown.as_bytes())
            .with_context(|| format!("failed to write {}", target_path.display()))?;
        Some(
            fs::metadata(&target_path)
                .with_context(|| format!("failed to stat {}", target_path.display()))?
                .len(),
        )
    };

    Ok(ProjectRunOutput {
        session_id,
        source_path,
        target_path,
        lane: opts.lane,
        embed_collection,
        message_count: projection.message_count,
        filtered_noise_count: projection.filtered_noise_count,
        tool_call_count: projection.tool_calls.len(),
        truncated: projection.truncated,
        written_bytes,
    })
}

pub fn run_and_mark_embed_pending(
    paths: &MoonPaths,
    state: &mut MoonState,
    opts: &ProjectRunOptions,
) -> Result<ProjectRunOutput> {
    let output = run(paths, opts)?;
    if !opts.dry_run {
        mark_embed_maintenance_pending(state, &output.embed_collection, now_epoch_secs()?);
    }
    Ok(output)
}

#[derive(Debug, Clone, Default)]
pub struct HotCachePruneOutput {
    pub removed_docs: usize,
    pub removed_index_entries: usize,
    pub removed_distill_entries: usize,
    pub removed_pending_hot_collections: usize,
}

pub fn prune_hot_cache_for_session(
    paths: &MoonPaths,
    state: &mut MoonState,
    active_session_id: &str,
) -> Result<HotCachePruneOutput> {
    let mut docs = Vec::new();
    gather_markdown_docs(&paths.mds_dir, &mut docs)?;

    let mut removed_docs = Vec::new();
    for path in docs {
        let session_id = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        if session_id.is_empty() || session_id == active_session_id {
            continue;
        }
        match fs::remove_file(&path) {
            Ok(_) => removed_docs.push(path),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("failed to remove hot cache {}", path.display()));
            }
        }
    }

    let removed_paths = removed_docs
        .iter()
        .map(|path| path.display().to_string())
        .collect::<BTreeSet<_>>();

    let removed_index_entries = remove_embedded_projection_paths(state, &removed_paths);

    let distilled_before = state.distilled_archives.len();
    state
        .distilled_archives
        .retain(|path, _| !removed_paths.contains(path));
    let removed_distill_entries = distilled_before.saturating_sub(state.distilled_archives.len());

    let keep_hot_collection = hot_embed_collection_for_session(active_session_id);
    let pending_before = state.pending_embed_collections.len();
    state.pending_embed_collections.retain(|collection, _| {
        if !is_hot_embed_collection(collection) {
            return true;
        }
        collection == &keep_hot_collection || collection == HOT_EMBED_COLLECTION_FALLBACK
    });
    let removed_pending_hot_collections =
        pending_before.saturating_sub(state.pending_embed_collections.len());

    state
        .hot_projection_cursors
        .retain(|session_id, _| session_id == active_session_id);

    let stale_hot_collections = state
        .managed_hot_collections
        .keys()
        .filter(|collection| is_hot_embed_collection(collection))
        .filter(|collection| *collection != &keep_hot_collection)
        .cloned()
        .collect::<Vec<_>>();
    for collection in stale_hot_collections {
        remove_embedded_projection_collection(state, &collection);
    }

    Ok(HotCachePruneOutput {
        removed_docs: removed_docs.len(),
        removed_index_entries,
        removed_distill_entries,
        removed_pending_hot_collections,
    })
}

fn gather_markdown_docs(root: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    gather_files_with_extension(root, "md", true, out)
}

fn select_source_path(paths: &MoonPaths, opts: &ProjectRunOptions) -> Result<PathBuf> {
    if let Some(source_path) = opts.source_path.as_deref()
        && !source_path.trim().is_empty()
    {
        let path = PathBuf::from(source_path.trim());
        ensure_readable_file(&path, "project")?;
        return Ok(path);
    }

    if let Some(session_id) = opts.session_id.as_deref()
        && !session_id.trim().is_empty()
    {
        let path = paths.raw_dir.join(format!("{}.jsonl", session_id.trim()));
        ensure_readable_file(&path, "project")?;
        return Ok(path);
    }

    latest_raw_file(&paths.raw_dir)?
        .ok_or_else(|| anyhow::anyhow!("no raw sources found under {}", paths.raw_dir.display()))
}

fn latest_raw_file(raw_dir: &Path) -> Result<Option<PathBuf>> {
    latest_file_with_extension(raw_dir, "jsonl", false)
}

fn render_projection_markdown(
    session_id: &str,
    source_path: &Path,
    data: &ProjectionData,
) -> String {
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str("moon_projection: 1\n");
    out.push_str(&format!(
        "session_id: {}\n",
        serde_json::to_string(session_id).unwrap_or_else(|_| "\"session\"".to_string())
    ));
    out.push_str(&format!(
        "source_path: {}\n",
        serde_json::to_string(&source_path.display().to_string())
            .unwrap_or_else(|_| "\"\"".to_string())
    ));
    out.push_str(&format!("message_count: {}\n", data.message_count));
    out.push_str(&format!(
        "filtered_noise_count: {}\n",
        data.filtered_noise_count
    ));
    out.push_str(&format!("truncated: {}\n", data.truncated));
    out.push_str("---\n\n");

    let user_lines = collect_turn_lines(data, "user");
    let assistant_lines = collect_turn_lines(data, "assistant");
    let tool_sections = collect_tool_sections(data);

    out.push_str("## Conversations\n\n");
    out.push_str("### User Queries\n");
    append_bullets(&mut out, &user_lines);
    out.push('\n');

    out.push_str("### Assistant Responses\n");
    append_bullets(&mut out, &assistant_lines);
    out.push('\n');

    out.push_str("## Tool Activity\n");
    if tool_sections.is_empty() {
        out.push_str("- none\n");
    } else {
        for (tool_name, lines) in tool_sections {
            out.push_str(&format!("### {tool_name}\n"));
            append_bullets(&mut out, &lines);
        }
    }

    out
}

fn collect_turn_lines(data: &ProjectionData, role: &str) -> Vec<String> {
    data.entries
        .iter()
        .filter(|entry| entry.role == role && entry.tool_name.is_none())
        .filter_map(|entry| {
            normalize_projection_text(&entry.content).map(|text| {
                if let Some(timestamp) = render_timestamp(entry.timestamp_epoch) {
                    format!("[{timestamp}] {text}")
                } else {
                    text
                }
            })
        })
        .collect()
}

fn collect_tool_sections(data: &ProjectionData) -> BTreeMap<String, Vec<String>> {
    let mut sections = BTreeMap::<String, Vec<String>>::new();

    for entry in &data.entries {
        match entry.role.as_str() {
            "assistant" if entry.tool_name.is_some() => {
                let name = entry
                    .tool_name
                    .clone()
                    .unwrap_or_else(|| "tool".to_string());
                let line = render_tool_line(entry);
                if !line.is_empty() {
                    sections.entry(name).or_default().push(line);
                }
            }
            "toolResult" => {
                if let Some(line) = render_tool_result_line(entry) {
                    sections
                        .entry("toolResult".to_string())
                        .or_default()
                        .push(line);
                }
            }
            _ => {}
        }
    }

    sections
}

fn render_tool_line(entry: &ProjectionEntry) -> String {
    let mut parts = Vec::new();
    if let Some(target) = entry.tool_target.as_deref()
        && let Some(cleaned) = normalize_projection_text(target)
    {
        parts.push(format!("target={cleaned}"));
    }
    if let Some(cleaned) = normalize_projection_text(&entry.content) {
        parts.push(cleaned);
    }
    if let Some(result) = entry.coupled_result.as_deref()
        && let Some(cleaned) = normalize_projection_text(result)
    {
        parts.push(format!("result={cleaned}"));
    }

    let body = if parts.is_empty() {
        entry
            .tool_name
            .as_deref()
            .map(|name| format!("used `{name}`"))
            .unwrap_or_default()
    } else {
        parts.join(" | ")
    };

    if body.is_empty() {
        String::new()
    } else if let Some(timestamp) = render_timestamp(entry.timestamp_epoch) {
        format!("[{timestamp}] {body}")
    } else {
        body
    }
}

fn render_tool_result_line(entry: &ProjectionEntry) -> Option<String> {
    let text = normalize_projection_text(&entry.content)?;
    Some(
        if let Some(timestamp) = render_timestamp(entry.timestamp_epoch) {
            format!("[{timestamp}] {text}")
        } else {
            text
        },
    )
}

fn append_bullets(out: &mut String, lines: &[String]) {
    if lines.is_empty() {
        out.push_str("- none\n");
        return;
    }

    for line in lines {
        out.push_str("- ");
        out.push_str(line);
        out.push('\n');
    }
}

fn normalize_projection_text(raw: &str) -> Option<String> {
    let collapsed = raw
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let collapsed = collapsed.trim();
    if collapsed.is_empty() {
        None
    } else if should_suppress_projection_text(collapsed) {
        None
    } else {
        Some(truncate_with_ellipsis(collapsed, 240))
    }
}

fn should_suppress_projection_text(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    if lower.contains("more characters truncated") {
        return true;
    }
    if lower.contains("providers declare their own auth/readiness")
        || lower.contains("use action=\"list\" to inspect registered providers")
        || lower.contains(
            "generated images are delivered automatically from the tool result as media paths",
        )
    {
        return true;
    }
    longest_base64ish_run(text) >= 320
}

fn longest_base64ish_run(text: &str) -> usize {
    let mut longest = 0usize;
    let mut current = 0usize;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '+' | '/' | '=' | '_' | '-') {
            current += 1;
            if current > longest {
                longest = current;
            }
        } else {
            current = 0;
        }
    }
    longest
}

fn render_timestamp(epoch: Option<u64>) -> Option<String> {
    let epoch = epoch?;
    let dt = Utc.timestamp_opt(epoch as i64, 0).single()?;
    Some(dt.to_rfc3339_opts(SecondsFormat::Secs, true))
}

#[cfg(test)]
mod tests {
    #[test]
    fn normalize_projection_text_filters_noise_blobs_and_boilerplate() {
        assert!(super::normalize_projection_text(
            "Providers declare their own auth/readiness; use action=\"list\" to inspect registered providers."
        )
        .is_none());
        assert!(
            super::normalize_projection_text("x [... 107543 more characters truncated]").is_none()
        );
        assert!(super::normalize_projection_text(&format!("result={}", "A".repeat(420))).is_none());
    }

    #[test]
    fn normalize_projection_text_keeps_meaningful_content() {
        let text = super::normalize_projection_text(
            "Found target thread in session 734c93a6-0ae0-4c40-90b6-be5cacbcd43f line 39.",
        )
        .expect("should keep meaningful line");
        assert!(text.contains("Found target thread"));
    }
}
