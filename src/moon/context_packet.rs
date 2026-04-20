use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::moon::config::MoonContextPacketConfig;
use crate::moon::distill::{ProjectionData, ProjectionEntry, extract_projection_data};
use crate::moon::files::{file_epoch_secs, gather_files_with_extension};
use crate::moon::paths::MoonPaths;
use crate::moon::qmd;
use crate::moon::state::{LIBRARY_EMBED_COLLECTION, MoonState, hot_embed_collection_for_session};
use crate::moon::util::{now_epoch_secs, truncate_with_ellipsis};

const MAX_QUERY_CHARS: usize = 240;
const MAX_DOC_LINE_CHARS: usize = 220;
const MAX_QMD_SNIPPET_CHARS: usize = 220;

#[derive(Debug, Clone)]
pub struct ContextPacketInput {
    pub session_id: String,
    pub raw_source_path: PathBuf,
    pub cleanse_summary_path: Option<PathBuf>,
    pub replay_has_compaction_summary: bool,
}

#[derive(Debug, Clone)]
pub struct ContextPacketOutput {
    pub session_id: String,
    pub content: String,
    pub packet_at_epoch_secs: u64,
    pub candidate_count: usize,
    pub cache_hit: bool,
    pub generation: String,
    pub query: String,
}

#[derive(Debug, Clone)]
struct PacketCandidate {
    source_kind: &'static str,
    source_label: String,
    text: String,
    score: i64,
}

#[derive(Debug, Clone)]
struct PacketSections {
    current_goal: Vec<String>,
    active_work: Vec<String>,
    relevant_memory: Vec<String>,
    open_items: Vec<String>,
    evidence: Vec<String>,
    candidate_count: usize,
}

pub fn output_path(paths: &MoonPaths, session_id: &str) -> PathBuf {
    paths.context_packet_dir.join(format!("{session_id}.md"))
}

pub fn write_context_packet_output(
    paths: &MoonPaths,
    session_id: &str,
    content: &str,
) -> Result<PathBuf> {
    fs::create_dir_all(&paths.context_packet_dir)
        .with_context(|| format!("failed to create {}", paths.context_packet_dir.display()))?;
    let path = output_path(paths, session_id);
    fs::write(&path, content.as_bytes())
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

pub fn build_context_packet(
    paths: &MoonPaths,
    state: &MoonState,
    cfg: &MoonContextPacketConfig,
    input: &ContextPacketInput,
) -> Result<ContextPacketOutput> {
    let raw_source_path = input
        .raw_source_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("context packet raw source path is not valid UTF-8"))?;
    let projection = extract_projection_data(raw_source_path).with_context(|| {
        format!(
            "failed to build context packet from {}",
            input.raw_source_path.display()
        )
    })?;
    let query = build_query_text(&projection);
    let generation = build_packet_generation(paths, state, cfg, input, &projection, &query)?;
    let packet_path = output_path(paths, &input.session_id);
    if state.last_context_packet_session_id.as_deref() == Some(input.session_id.as_str())
        && state.last_context_packet_generation.as_deref() == Some(generation.as_str())
        && packet_path.is_file()
    {
        let content = fs::read_to_string(&packet_path)
            .with_context(|| format!("failed to read {}", packet_path.display()))?;
        return Ok(ContextPacketOutput {
            session_id: input.session_id.clone(),
            content,
            packet_at_epoch_secs: state.last_context_packet_epoch_secs.unwrap_or(0),
            candidate_count: state.last_context_packet_candidate_count.unwrap_or(0),
            cache_hit: true,
            generation,
            query,
        });
    }

    let query_terms = query_terms(&query, &projection);
    let sections = build_sections(paths, state, cfg, input, &projection, &query_terms)?;
    let content = render_packet(&sections, cfg.max_chars);

    Ok(ContextPacketOutput {
        session_id: input.session_id.clone(),
        content,
        packet_at_epoch_secs: now_epoch_secs()?,
        candidate_count: sections.candidate_count,
        cache_hit: false,
        generation,
        query,
    })
}

fn build_sections(
    paths: &MoonPaths,
    state: &MoonState,
    cfg: &MoonContextPacketConfig,
    input: &ContextPacketInput,
    projection: &ProjectionData,
    query_terms: &[String],
) -> Result<PacketSections> {
    let current_goal = latest_goal_lines(projection, 2);
    let active_work = recent_activity_lines(projection, 5);
    let mut relevant_memory = Vec::new();
    let mut open_items = Vec::new();

    let mut candidates = collect_candidates(paths, state, cfg, input, projection, query_terms)?;
    let candidate_count = candidates.len();
    candidates.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.source_label.cmp(&right.source_label))
            .then_with(|| left.text.cmp(&right.text))
    });

    let mut used_text = BTreeSet::new();
    for candidate in &candidates {
        let key = normalize_for_dedupe(&candidate.text);
        if !used_text.insert(key) {
            continue;
        }
        if relevant_memory.len() < 6 && candidate.source_kind != "hot" {
            relevant_memory.push(format!(
                "[{}] {}",
                candidate.source_label,
                truncate_with_ellipsis(&candidate.text, MAX_DOC_LINE_CHARS)
            ));
        }
        if open_items.len() < 6 && looks_actionable(&candidate.text) {
            open_items.push(truncate_with_ellipsis(&candidate.text, MAX_DOC_LINE_CHARS));
        }
    }

    if open_items.is_empty() {
        open_items = extract_open_items_from_projection(projection, 4);
    }

    let evidence = candidates
        .into_iter()
        .take(cfg.max_candidates.min(candidate_count.max(1)))
        .map(|candidate| {
            format!(
                "[{}] {}",
                candidate.source_label,
                truncate_with_ellipsis(&candidate.text, MAX_DOC_LINE_CHARS)
            )
        })
        .collect::<Vec<_>>();

    Ok(PacketSections {
        current_goal,
        active_work,
        relevant_memory,
        open_items,
        evidence,
        candidate_count,
    })
}

fn collect_candidates(
    paths: &MoonPaths,
    state: &MoonState,
    cfg: &MoonContextPacketConfig,
    input: &ContextPacketInput,
    projection: &ProjectionData,
    query_terms: &[String],
) -> Result<Vec<PacketCandidate>> {
    let mut out = Vec::new();
    let hot_candidates = projection
        .entries
        .iter()
        .rev()
        .filter_map(render_projection_candidate)
        .take(6)
        .collect::<Vec<_>>();
    for text in hot_candidates.into_iter().rev() {
        out.push(PacketCandidate {
            source_kind: "hot",
            source_label: "hot".to_string(),
            score: score_text("hot", &text, query_terms),
            text,
        });
    }

    if !input.replay_has_compaction_summary
        && let Some(path) = input.cleanse_summary_path.as_ref()
        && path.is_file()
    {
        let body = read_markdown_body(path)?;
        for line in select_doc_lines(&body, query_terms, 4) {
            out.push(PacketCandidate {
                source_kind: "cleanse",
                source_label: "cleanse".to_string(),
                score: score_text("cleanse", &line, query_terms),
                text: line,
            });
        }
    }

    if paths.memory_file.is_file() {
        let body = read_markdown_body(&paths.memory_file)?;
        for line in select_doc_lines(&body, query_terms, 4) {
            out.push(PacketCandidate {
                source_kind: "memory",
                source_label: "memory".to_string(),
                score: score_text("memory", &line, query_terms),
                text: line,
            });
        }
    }

    for path in recent_markdown_files(&paths.memory_dir, cfg.recent_memory_files)?
        .into_iter()
        .filter(|path| path != &paths.memory_file)
    {
        let body = read_markdown_body(&path)?;
        let label = short_source_label("memory", &path);
        for line in select_doc_lines(&body, query_terms, 2) {
            out.push(PacketCandidate {
                source_kind: "memory",
                source_label: label.clone(),
                score: score_text("memory", &line, query_terms),
                text: line,
            });
        }
    }

    for path in recent_markdown_files(&paths.mlib_dir, 3)? {
        let body = read_markdown_body(&path)?;
        let label = short_source_label("lib", &path);
        for line in select_doc_lines(&body, query_terms, 2) {
            out.push(PacketCandidate {
                source_kind: "library",
                source_label: label.clone(),
                score: score_text("library", &line, query_terms),
                text: line,
            });
        }
    }

    for (path, _) in recent_distill_paths(state, cfg.recent_distill_docs) {
        if !path.is_file() {
            continue;
        }
        let body = read_markdown_body(&path)?;
        let label = short_source_label("distill", &path);
        for line in select_doc_lines(&body, query_terms, 2) {
            out.push(PacketCandidate {
                source_kind: "distill",
                source_label: label.clone(),
                score: score_text("distill", &line, query_terms),
                text: line,
            });
        }
    }

    if cfg.qmd_limit > 0 {
        out.extend(collect_qmd_candidates(
            paths,
            &hot_embed_collection_for_session(&input.session_id),
            "qmd-hot",
            &build_qmd_query(projection, query_terms),
            cfg.qmd_limit,
            query_terms,
        ));
        out.extend(collect_qmd_candidates(
            paths,
            LIBRARY_EMBED_COLLECTION,
            "qmd-lib",
            &build_qmd_query(projection, query_terms),
            cfg.qmd_limit,
            query_terms,
        ));
    }

    Ok(dedup_candidates(out))
}

fn collect_qmd_candidates(
    paths: &MoonPaths,
    collection_name: &str,
    source_label: &str,
    query: &str,
    limit: usize,
    query_terms: &[String],
) -> Vec<PacketCandidate> {
    if query.trim().is_empty() {
        return Vec::new();
    }
    let Ok(exec) = qmd::recall_query(&paths.qmd_bin, collection_name, query, limit, Some(15))
    else {
        return Vec::new();
    };
    parse_qmd_hits(&exec.stdout)
        .into_iter()
        .map(|hit| PacketCandidate {
            source_kind: "qmd",
            source_label: if hit.source_label.is_empty() {
                source_label.to_string()
            } else {
                format!("{source_label}:{}", hit.source_label)
            },
            score: score_text("qmd", &hit.text, query_terms),
            text: hit.text,
        })
        .collect()
}

#[derive(Debug, Clone)]
struct ParsedQmdHit {
    source_label: String,
    text: String,
}

fn parse_qmd_hits(raw: &str) -> Vec<ParsedQmdHit> {
    let Ok(json) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    let Some(results) = json.get("results").and_then(Value::as_array) else {
        return Vec::new();
    };
    results
        .iter()
        .filter_map(|item| {
            let text = item
                .get("snippet")
                .and_then(Value::as_str)
                .or_else(|| item.get("text").and_then(Value::as_str))
                .map(|value| truncate_with_ellipsis(value.trim(), MAX_QMD_SNIPPET_CHARS))?;
            if text.trim().is_empty() {
                return None;
            }
            let source_label = item
                .get("path")
                .and_then(Value::as_str)
                .map(PathBuf::from)
                .as_ref()
                .map(|path| short_source_label("", path))
                .unwrap_or_default();
            Some(ParsedQmdHit { source_label, text })
        })
        .collect()
}

fn build_packet_generation(
    paths: &MoonPaths,
    state: &MoonState,
    cfg: &MoonContextPacketConfig,
    input: &ContextPacketInput,
    projection: &ProjectionData,
    query: &str,
) -> Result<String> {
    let memory_files = recent_markdown_files(&paths.memory_dir, cfg.recent_memory_files)?
        .into_iter()
        .map(|path| format!("{}:{}", path.display(), file_epoch_secs(&path)))
        .collect::<Vec<_>>()
        .join(",");
    let distill_files = recent_distill_paths(state, cfg.recent_distill_docs)
        .into_iter()
        .map(|(path, epoch)| format!("{}:{epoch}", path.display()))
        .collect::<Vec<_>>()
        .join(",");

    Ok(format!(
        "session={}::raw={}::cleanse={}::memory={}::memory_files={}::distill={}::embed={}::query={}::topics={}::cfg={}/{}/{}/{}::replay={}",
        input.session_id,
        file_epoch_secs(&input.raw_source_path),
        input
            .cleanse_summary_path
            .as_ref()
            .map(|path| file_epoch_secs(path))
            .unwrap_or(0),
        file_epoch_secs(&paths.memory_file),
        memory_files,
        distill_files,
        state.last_embed_trigger_epoch_secs.unwrap_or(0),
        truncate_with_ellipsis(query, MAX_QUERY_CHARS),
        projection.topics.join(","),
        cfg.max_chars,
        cfg.max_candidates,
        cfg.qmd_limit,
        cfg.recent_memory_files,
        input.replay_has_compaction_summary
    ))
}

fn build_query_text(projection: &ProjectionData) -> String {
    let latest_users = latest_goal_lines(projection, 2);
    let mut parts = latest_users;
    if parts.is_empty() {
        parts.extend(projection.keywords.iter().take(6).cloned());
    }
    if parts.is_empty() {
        parts.extend(projection.topics.iter().take(4).cloned());
    }
    truncate_with_ellipsis(&parts.join(" "), MAX_QUERY_CHARS)
}

fn build_qmd_query(projection: &ProjectionData, query_terms: &[String]) -> String {
    let mut terms = query_terms.iter().take(10).cloned().collect::<Vec<_>>();
    if terms.is_empty() {
        terms.extend(projection.keywords.iter().take(6).cloned());
    }
    truncate_with_ellipsis(&terms.join(" "), MAX_QUERY_CHARS)
}

fn query_terms(query: &str, projection: &ProjectionData) -> Vec<String> {
    let mut out = tokenize(query);
    if out.len() < 4 {
        out.extend(
            projection
                .keywords
                .iter()
                .flat_map(|value| tokenize(value))
                .collect::<Vec<_>>(),
        );
    }
    out.sort();
    out.dedup();
    out
}

fn tokenize(raw: &str) -> Vec<String> {
    raw.split(|ch: char| !ch.is_ascii_alphanumeric())
        .map(|part| part.trim().to_ascii_lowercase())
        .filter(|part| part.len() >= 3)
        .filter(|part| {
            !matches!(
                part.as_str(),
                "the"
                    | "and"
                    | "for"
                    | "that"
                    | "with"
                    | "from"
                    | "this"
                    | "keep"
                    | "moon"
                    | "openclaw"
                    | "into"
                    | "have"
                    | "will"
            )
        })
        .collect()
}

fn latest_goal_lines(projection: &ProjectionData, limit: usize) -> Vec<String> {
    projection
        .entries
        .iter()
        .rev()
        .filter(|entry| entry.role == "user")
        .filter_map(|entry| clean_line(&entry.content))
        .take(limit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn recent_activity_lines(projection: &ProjectionData, limit: usize) -> Vec<String> {
    projection
        .entries
        .iter()
        .rev()
        .filter(|entry| entry.role != "user")
        .filter_map(render_projection_candidate)
        .take(limit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn render_projection_candidate(entry: &ProjectionEntry) -> Option<String> {
    let base = clean_line(&entry.content)?;
    let text = match entry.role.as_str() {
        "assistant" if entry.tool_name.is_some() => {
            let tool_name = entry.tool_name.as_deref().unwrap_or("tool");
            format!("Assistant used `{tool_name}`: {base}")
        }
        "assistant" => format!("Assistant: {base}"),
        "toolResult" => format!("Tool result: {base}"),
        "system" => base,
        _ => base,
    };
    Some(truncate_with_ellipsis(&text, MAX_DOC_LINE_CHARS))
}

fn extract_open_items_from_projection(projection: &ProjectionData, limit: usize) -> Vec<String> {
    projection
        .entries
        .iter()
        .rev()
        .filter_map(|entry| clean_line(&entry.content))
        .filter(|line| looks_actionable(line))
        .take(limit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn recent_markdown_files(root: &Path, limit: usize) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    gather_files_with_extension(root, "md", true, &mut files)?;
    files.sort_by(|left, right| {
        file_epoch_secs(right)
            .cmp(&file_epoch_secs(left))
            .then_with(|| left.cmp(right))
    });
    files.truncate(limit);
    Ok(files)
}

fn recent_distill_paths(state: &MoonState, limit: usize) -> Vec<(PathBuf, u64)> {
    let mut paths = state
        .distilled_archives
        .iter()
        .map(|(path, epoch)| (PathBuf::from(path), *epoch))
        .collect::<Vec<_>>();
    paths.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    paths.truncate(limit);
    paths
}

fn read_markdown_body(path: &Path) -> Result<String> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    Ok(strip_frontmatter(&raw).trim().to_string())
}

fn strip_frontmatter(raw: &str) -> &str {
    let Some(rest) = raw.strip_prefix("---\n") else {
        return raw;
    };
    let Some(idx) = rest.find("\n---\n") else {
        return raw;
    };
    &rest[idx + 5..]
}

fn select_doc_lines(body: &str, query_terms: &[String], limit: usize) -> Vec<String> {
    let mut scored = body
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| !line.starts_with("```"))
        .map(|line| line.trim_start_matches("- ").trim())
        .filter(|line| !line.is_empty())
        .map(|line| {
            let cleaned = truncate_with_ellipsis(line, MAX_DOC_LINE_CHARS);
            let score = overlap_score(&cleaned, query_terms)
                + if looks_actionable(&cleaned) { 4 } else { 0 }
                + if cleaned.starts_with('#') { 1 } else { 0 };
            (score, cleaned)
        })
        .collect::<Vec<_>>();
    scored.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    let mut out = scored
        .into_iter()
        .filter(|(score, _)| *score > 0)
        .map(|(_, line)| line)
        .take(limit)
        .collect::<Vec<_>>();
    if out.is_empty() {
        out = body
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .take(limit)
            .map(|line| {
                truncate_with_ellipsis(line.trim_start_matches("- ").trim(), MAX_DOC_LINE_CHARS)
            })
            .collect();
    }
    out
}

fn render_packet(sections: &PacketSections, max_chars: usize) -> String {
    let mut out = String::new();
    out.push_str("# Moon Active Context\n\n");
    append_section(&mut out, "Current Goal", &sections.current_goal);
    append_section(&mut out, "Active Work", &sections.active_work);
    append_section(&mut out, "Relevant Memory", &sections.relevant_memory);
    append_section(&mut out, "Open Items", &sections.open_items);
    append_section(&mut out, "Evidence", &sections.evidence);
    truncate_with_ellipsis(out.trim_end(), max_chars)
}

fn append_section(out: &mut String, title: &str, lines: &[String]) {
    if lines.is_empty() {
        return;
    }
    out.push_str("## ");
    out.push_str(title);
    out.push('\n');
    for line in lines {
        out.push_str("- ");
        out.push_str(line);
        out.push('\n');
    }
    out.push('\n');
}

fn short_source_label(prefix: &str, path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("doc")
        .trim();
    if prefix.is_empty() {
        stem.to_string()
    } else if stem.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}:{stem}")
    }
}

fn clean_line(raw: &str) -> Option<String> {
    let cleaned = raw
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        None
    } else {
        Some(truncate_with_ellipsis(cleaned, MAX_DOC_LINE_CHARS))
    }
}

fn score_text(kind: &str, text: &str, query_terms: &[String]) -> i64 {
    let base = match kind {
        "cleanse" => 90,
        "memory" => 75,
        "library" => 65,
        "distill" => 60,
        "qmd" => 55,
        "hot" => 50,
        _ => 40,
    };
    i64::from(
        base + overlap_score(text, query_terms) * 5 + if looks_actionable(text) { 10 } else { 0 },
    )
}

fn overlap_score(text: &str, query_terms: &[String]) -> i32 {
    let lower = text.to_ascii_lowercase();
    query_terms
        .iter()
        .filter(|term| lower.contains(term.as_str()))
        .count() as i32
}

fn looks_actionable(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("todo")
        || lower.contains("next")
        || lower.contains("follow")
        || lower.contains("open item")
        || lower.contains("open task")
        || lower.contains("blocker")
        || lower.contains("risk")
        || lower.contains("pending")
        || lower.contains("action")
}

fn dedup_candidates(candidates: Vec<PacketCandidate>) -> Vec<PacketCandidate> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for candidate in candidates {
        let key = normalize_for_dedupe(&candidate.text);
        if key.is_empty() || !seen.insert(key) {
            continue;
        }
        out.push(candidate);
    }
    out
}

fn normalize_for_dedupe(raw: &str) -> String {
    raw.split_whitespace()
        .map(|part| part.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::{ContextPacketInput, build_context_packet};
    use crate::moon::config::MoonContextPacketConfig;
    use crate::moon::paths::MoonPaths;
    use crate::moon::state::MoonState;
    use serde_json::json;
    use std::fs;
    use std::path::Path;
    use tempfile::tempdir;

    fn test_paths(root: &Path) -> MoonPaths {
        MoonPaths {
            moon_home: root.to_path_buf(),
            raw_dir: root.join("raw"),
            mds_dir: root.join("mds"),
            mlib_dir: root.join("mlib"),
            cleanse_dir: root.join("cleanse"),
            memory_dir: root.join("memory"),
            memory_file: root.join("MEMORY.md"),
            logs_dir: root.join("logs"),
            context_engine_dir: root.join("mce"),
            context_packet_dir: root.join("mcp"),
            openclaw_sessions_dir: root.join("sessions"),
            qmd_bin: root.join("bin/qmd"),
            qmd_db: root.join("qmd.sqlite"),
            qmd_config_dir: root.join("qmd-config"),
            moon_home_is_explicit: true,
        }
    }

    #[test]
    fn context_packet_prefers_message_lane_material_and_omits_frontmatter() {
        let tmp = tempdir().expect("tempdir");
        let paths = test_paths(tmp.path());
        fs::create_dir_all(&paths.raw_dir).expect("mkdir raw");
        fs::create_dir_all(&paths.memory_dir).expect("mkdir memory");
        fs::create_dir_all(&paths.cleanse_dir).expect("mkdir cleanse");
        fs::write(
            paths.raw_dir.join("s1.jsonl"),
            format!(
                "{}\n{}\n{}\n",
                json!({"message":{"role":"user","content":[{"type":"text","text":"Keep the Moon packet in the messages lane."}]}}),
                json!({"message":{"role":"assistant","content":[{"type":"text","text":"Moon should keep systemPromptAddition empty."}]}}),
                json!({"message":{"role":"assistant","content":[{"type":"text","text":"Next step: wire deterministic retrieval first."}]}})
            ),
        )
        .expect("write raw");
        fs::write(
            &paths.memory_file,
            "# MEMORY\n- Decision: use a bounded Moon packet.\n- Next: avoid duplicate cleanse summaries.\n",
        )
        .expect("write memory");
        let cleanse_path = paths.cleanse_dir.join("s1.md");
        fs::write(
            &cleanse_path,
            "---\nmoon_cleanse: 1\n---\n# Cleanse Summary\n- Keep packet injection in messages.\n",
        )
        .expect("write cleanse");

        let output = build_context_packet(
            &paths,
            &MoonState::default(),
            &MoonContextPacketConfig::default(),
            &ContextPacketInput {
                session_id: "s1".to_string(),
                raw_source_path: paths.raw_dir.join("s1.jsonl"),
                cleanse_summary_path: Some(cleanse_path),
                replay_has_compaction_summary: true,
            },
        )
        .expect("build packet");

        assert!(output.content.contains("# Moon Active Context"));
        assert!(
            output
                .content
                .contains("Keep the Moon packet in the messages lane.")
        );
        assert!(output.content.contains("systemPromptAddition"));
        assert!(!output.content.contains("moon_cleanse: 1"));
        assert!(
            !output
                .content
                .contains("Keep packet injection in messages.")
        );
    }
}
