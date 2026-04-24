use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::moon::config::MoonContextPacketConfig;
use crate::moon::distill::{ProjectionData, ProjectionEntry};
use crate::moon::files::{file_epoch_secs, gather_files_with_extension};
use crate::moon::paths::MoonPaths;
use crate::moon::qmd;
use crate::moon::state::{LIBRARY_EMBED_COLLECTION, MoonState, hot_embed_collection_for_session};
use crate::moon::util::{now_epoch_secs, truncate_with_ellipsis};

const MAX_QUERY_CHARS: usize = 240;
const MAX_DOC_LINE_CHARS: usize = 220;
const MAX_QMD_SNIPPET_CHARS: usize = 220;
const PRIMARY_SOURCE_LIMIT: usize = 4;
const FALLBACK_SOURCE_LIMIT: usize = 2;

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
    pub primary_source_family: String,
    pub fallback_source: Option<String>,
    pub source_read_count: usize,
    pub qmd_query_count: usize,
    pub coverage_decision: String,
    pub coverage_reason: String,
    pub positive_candidate_count: usize,
    pub top_score: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceFamily {
    Hot,
    Memory,
    Library,
    Distill,
    Semantic,
}

impl SourceFamily {
    fn as_str(self) -> &'static str {
        match self {
            Self::Hot => "hot",
            Self::Memory => "memory",
            Self::Library => "library",
            Self::Distill => "distill",
            Self::Semantic => "semantic",
        }
    }
}

#[derive(Debug, Clone)]
struct PacketCandidate {
    source_kind: &'static str,
    source_label: String,
    text: String,
    score: i64,
}

#[derive(Debug, Clone)]
struct CoverageReport {
    decision: &'static str,
    reason: String,
}

#[derive(Debug, Clone)]
struct PacketSections {
    current_goal: Vec<String>,
    active_work: Vec<String>,
    relevant_memory: Vec<String>,
    open_items: Vec<String>,
    evidence: Vec<String>,
    candidate_count: usize,
    coverage: CoverageReport,
}

#[derive(Debug, Clone)]
struct PacketBuild {
    sections: PacketSections,
    fallback_source: Option<String>,
    source_read_count: usize,
    qmd_query_count: usize,
    positive_candidate_count: usize,
    top_score: i64,
}

#[derive(Debug, Clone)]
struct SelectedDocLine {
    text: String,
    score: i32,
}

#[derive(Debug, Clone)]
struct SourceCollectionSummary {
    candidate_count: usize,
    positive_count: usize,
}

impl SourceCollectionSummary {
    fn empty() -> Self {
        Self {
            candidate_count: 0,
            positive_count: 0,
        }
    }

    fn note_candidate(&mut self) {
        self.candidate_count = self.candidate_count.saturating_add(1);
        self.positive_count = self.positive_count.saturating_add(1);
    }
}

#[derive(Debug, Clone)]
struct CandidateCollector {
    primary_family: SourceFamily,
    fallback_source: Option<String>,
    source_read_count: usize,
    qmd_query_count: usize,
    candidates: BTreeMap<String, PacketCandidate>,
}

impl CandidateCollector {
    fn new(primary_family: SourceFamily) -> Self {
        Self {
            primary_family,
            fallback_source: None,
            source_read_count: 0,
            qmd_query_count: 0,
            candidates: BTreeMap::new(),
        }
    }

    fn note_file_read(&mut self) {
        self.source_read_count = self.source_read_count.saturating_add(1);
    }

    fn note_qmd_query(&mut self) {
        self.qmd_query_count = self.qmd_query_count.saturating_add(1);
    }

    fn mark_fallback<S: Into<String>>(&mut self, source: S) {
        if self.fallback_source.is_none() {
            self.fallback_source = Some(source.into());
        }
    }

    fn add_candidate(&mut self, candidate: PacketCandidate) {
        let key = normalize_for_dedupe(&candidate.text);
        if key.is_empty() {
            return;
        }
        match self.candidates.get(&key) {
            Some(existing) if !candidate_is_better(self.primary_family, &candidate, existing) => {}
            _ => {
                self.candidates.insert(key, candidate);
            }
        }
    }

    fn into_sorted_candidates(self) -> Vec<PacketCandidate> {
        let mut candidates = self.candidates.into_values().collect::<Vec<_>>();
        let primary_family = self.primary_family;
        candidates.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| {
                    candidate_source_preference(primary_family, right.source_kind).cmp(
                        &candidate_source_preference(primary_family, left.source_kind),
                    )
                })
                .then_with(|| left.source_label.cmp(&right.source_label))
                .then_with(|| left.text.cmp(&right.text))
        });
        candidates
    }
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

#[cfg(test)]
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
    let projection =
        crate::moon::distill::extract_projection_data(raw_source_path).with_context(|| {
            format!(
                "failed to build context packet from {}",
                input.raw_source_path.display()
            )
        })?;
    build_context_packet_from_projection(paths, state, cfg, input, &projection)
}

pub fn build_context_packet_from_projection(
    paths: &MoonPaths,
    state: &MoonState,
    cfg: &MoonContextPacketConfig,
    input: &ContextPacketInput,
    projection: &ProjectionData,
) -> Result<ContextPacketOutput> {
    let query = build_query_text(projection);
    let primary_family = route_source_family(&query, projection);
    let generation = build_packet_generation(paths, state, cfg, input, projection, &query)?;
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
            primary_source_family: primary_family.as_str().to_string(),
            fallback_source: None,
            source_read_count: 0,
            qmd_query_count: 0,
            coverage_decision: "cached".to_string(),
            coverage_reason: "reused previous packet generation".to_string(),
            positive_candidate_count: 0,
            top_score: 0,
        });
    }

    let query_terms = query_terms(&query, projection);
    let build = build_sections(
        paths,
        state,
        cfg,
        input,
        projection,
        &query_terms,
        primary_family,
    )?;
    let content = render_packet(&build.sections, cfg.max_chars);

    Ok(ContextPacketOutput {
        session_id: input.session_id.clone(),
        content,
        packet_at_epoch_secs: now_epoch_secs()?,
        candidate_count: build.sections.candidate_count,
        cache_hit: false,
        generation,
        query,
        primary_source_family: primary_family.as_str().to_string(),
        fallback_source: build.fallback_source,
        source_read_count: build.source_read_count,
        qmd_query_count: build.qmd_query_count,
        coverage_decision: build.sections.coverage.decision.to_string(),
        coverage_reason: build.sections.coverage.reason,
        positive_candidate_count: build.positive_candidate_count,
        top_score: build.top_score,
    })
}

fn build_sections(
    paths: &MoonPaths,
    state: &MoonState,
    cfg: &MoonContextPacketConfig,
    input: &ContextPacketInput,
    projection: &ProjectionData,
    query_terms: &[String],
    primary_family: SourceFamily,
) -> Result<PacketBuild> {
    let current_goal = latest_goal_lines(projection, 2);
    let active_work = recent_activity_lines(projection, query_terms, 5);
    let mut relevant_memory = Vec::new();
    let mut open_items = Vec::new();

    let collector = collect_candidates(
        paths,
        state,
        cfg,
        input,
        projection,
        query_terms,
        primary_family,
    )?;
    let fallback_source = collector.fallback_source.clone();
    let source_read_count = collector.source_read_count;
    let qmd_query_count = collector.qmd_query_count;
    let candidates = collector.into_sorted_candidates();
    let candidate_count = candidates.len();
    let positive_candidate_count = candidate_count;
    let top_score = candidates
        .iter()
        .map(|candidate| candidate.score)
        .max()
        .unwrap_or(0);
    let coverage = evaluate_coverage(
        primary_family,
        fallback_source.as_deref(),
        candidate_count,
        top_score,
    );

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

    Ok(PacketBuild {
        sections: PacketSections {
            current_goal,
            active_work,
            relevant_memory,
            open_items,
            evidence,
            candidate_count,
            coverage,
        },
        fallback_source,
        source_read_count,
        qmd_query_count,
        positive_candidate_count,
        top_score,
    })
}

fn collect_candidates(
    paths: &MoonPaths,
    state: &MoonState,
    cfg: &MoonContextPacketConfig,
    input: &ContextPacketInput,
    projection: &ProjectionData,
    query_terms: &[String],
    primary_family: SourceFamily,
) -> Result<CandidateCollector> {
    let mut collector = CandidateCollector::new(primary_family);
    let qmd_query = build_qmd_query(projection, query_terms);

    match primary_family {
        SourceFamily::Hot => {
            let summary = collect_hot_candidates(projection, query_terms, &mut collector);
            if summary.positive_count == 0
                && !input.replay_has_compaction_summary
                && let Some(path) = input.cleanse_summary_path.as_ref()
                && path.is_file()
            {
                let label = "cleanse".to_string();
                let fallback = collect_markdown_candidates_from_path(
                    path,
                    "cleanse",
                    &label,
                    query_terms,
                    FALLBACK_SOURCE_LIMIT,
                    &mut collector,
                )?;
                if fallback.candidate_count > 0 {
                    collector.mark_fallback(label);
                }
            }
        }
        SourceFamily::Memory => {
            let primary = collect_markdown_candidates_from_path(
                &paths.memory_file,
                "memory-file",
                "memory",
                query_terms,
                PRIMARY_SOURCE_LIMIT,
                &mut collector,
            )?;
            if primary.positive_count == 0
                && let Some(path) = newest_daily_memory_file(paths, cfg)?
            {
                let label = short_source_label("memory", &path);
                let fallback = collect_markdown_candidates_from_path(
                    &path,
                    "memory-daily",
                    &label,
                    query_terms,
                    FALLBACK_SOURCE_LIMIT,
                    &mut collector,
                )?;
                if fallback.candidate_count > 0 {
                    collector.mark_fallback(label);
                }
            }
        }
        SourceFamily::Library => {
            let primary = if let Some(path) = newest_library_doc(paths)? {
                let label = short_source_label("lib", &path);
                collect_markdown_candidates_from_path(
                    &path,
                    "library",
                    &label,
                    query_terms,
                    PRIMARY_SOURCE_LIMIT,
                    &mut collector,
                )?
            } else {
                SourceCollectionSummary::empty()
            };
            if primary.positive_count == 0 && cfg.qmd_limit > 0 {
                let fallback = collect_qmd_candidates(
                    paths,
                    LIBRARY_EMBED_COLLECTION,
                    "qmd-lib",
                    &qmd_query,
                    cfg.qmd_limit,
                    query_terms,
                    &mut collector,
                );
                if fallback.candidate_count > 0 {
                    collector.mark_fallback("qmd-lib");
                }
            }
        }
        SourceFamily::Distill => {
            let primary = if let Some((path, _)) = newest_distill_doc(state, cfg) {
                let label = short_source_label("distill", &path);
                collect_markdown_candidates_from_path(
                    &path,
                    "distill",
                    &label,
                    query_terms,
                    PRIMARY_SOURCE_LIMIT,
                    &mut collector,
                )?
            } else {
                SourceCollectionSummary::empty()
            };
            if primary.positive_count == 0
                && let Some(path) = newest_daily_memory_file(paths, cfg)?
            {
                let label = short_source_label("memory", &path);
                let fallback = collect_markdown_candidates_from_path(
                    &path,
                    "memory-daily",
                    &label,
                    query_terms,
                    FALLBACK_SOURCE_LIMIT,
                    &mut collector,
                )?;
                if fallback.candidate_count > 0 {
                    collector.mark_fallback(label);
                }
            }
        }
        SourceFamily::Semantic => {
            let hot = collect_qmd_candidates(
                paths,
                &hot_embed_collection_for_session(&input.session_id),
                "qmd-hot",
                &qmd_query,
                cfg.qmd_limit,
                query_terms,
                &mut collector,
            );
            if hot.positive_count == 0 && cfg.qmd_limit > 0 {
                let lib = collect_qmd_candidates(
                    paths,
                    LIBRARY_EMBED_COLLECTION,
                    "qmd-lib",
                    &qmd_query,
                    cfg.qmd_limit,
                    query_terms,
                    &mut collector,
                );
                if lib.candidate_count > 0 {
                    collector.mark_fallback("qmd-lib");
                }
            }
        }
    }

    Ok(collector)
}

fn collect_hot_candidates(
    projection: &ProjectionData,
    query_terms: &[String],
    collector: &mut CandidateCollector,
) -> SourceCollectionSummary {
    let hot_candidates = projection
        .entries
        .iter()
        .rev()
        .filter(|entry| entry.role != "user")
        .filter_map(render_projection_candidate)
        .take(6)
        .collect::<Vec<_>>();
    let mut summary = SourceCollectionSummary::empty();
    for text in hot_candidates.into_iter().rev() {
        if !candidate_is_relevant("hot", collector.primary_family, &text, query_terms) {
            continue;
        }
        let score = score_text("hot", &text, query_terms, collector.primary_family);
        summary.note_candidate();
        collector.add_candidate(PacketCandidate {
            source_kind: "hot",
            source_label: "hot".to_string(),
            score,
            text,
        });
    }
    summary
}

fn collect_markdown_candidates_from_path(
    path: &Path,
    source_kind: &'static str,
    source_label: &str,
    query_terms: &[String],
    limit: usize,
    collector: &mut CandidateCollector,
) -> Result<SourceCollectionSummary> {
    if !path.is_file() {
        return Ok(SourceCollectionSummary::empty());
    }
    collector.note_file_read();
    let body = read_markdown_body(path)?;
    let selected = select_doc_candidates(&body, query_terms, limit);
    let mut summary = SourceCollectionSummary::empty();
    for line in selected {
        if !candidate_is_relevant(
            source_kind,
            collector.primary_family,
            &line.text,
            query_terms,
        ) {
            continue;
        }
        let score = score_text(
            source_kind,
            &line.text,
            query_terms,
            collector.primary_family,
        );
        summary.note_candidate();
        collector.add_candidate(PacketCandidate {
            source_kind,
            source_label: source_label.to_string(),
            score,
            text: line.text,
        });
    }
    Ok(summary)
}

fn collect_qmd_candidates(
    paths: &MoonPaths,
    collection_name: &str,
    source_label: &str,
    query: &str,
    limit: usize,
    query_terms: &[String],
    collector: &mut CandidateCollector,
) -> SourceCollectionSummary {
    if query.trim().is_empty() {
        return SourceCollectionSummary::empty();
    }
    let Ok(exec) = qmd::recall_query(&paths.qmd_bin, collection_name, query, limit, Some(15))
    else {
        return SourceCollectionSummary::empty();
    };
    collector.note_qmd_query();
    let mut summary = SourceCollectionSummary::empty();
    for hit in parse_qmd_hits(&exec.stdout) {
        if !candidate_is_relevant("qmd", collector.primary_family, &hit.text, query_terms) {
            continue;
        }
        let score = score_text("qmd", &hit.text, query_terms, collector.primary_family);
        summary.note_candidate();
        collector.add_candidate(PacketCandidate {
            source_kind: "qmd",
            source_label: if hit.source_label.is_empty() {
                source_label.to_string()
            } else {
                format!("{source_label}:{}", hit.source_label)
            },
            score,
            text: hit.text,
        });
    }
    summary
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
    let primary_family = route_source_family(query, projection);
    let mut parts = vec![
        format!("session={}", input.session_id),
        format!("raw={}", file_epoch_secs(&input.raw_source_path)),
        format!(
            "cleanse={}",
            input
                .cleanse_summary_path
                .as_ref()
                .map(|path| file_epoch_secs(path))
                .unwrap_or(0)
        ),
        format!("embed={}", state.last_embed_trigger_epoch_secs.unwrap_or(0)),
        format!("query={}", truncate_with_ellipsis(query, MAX_QUERY_CHARS)),
        format!("topics={}", projection.topics.join(",")),
        format!("primary={}", primary_family.as_str()),
        format!(
            "cfg={}/{}/{}/{}",
            cfg.max_chars, cfg.max_candidates, cfg.qmd_limit, cfg.recent_memory_files
        ),
        format!("replay={}", input.replay_has_compaction_summary),
    ];

    match primary_family {
        SourceFamily::Hot => {
            if !input.replay_has_compaction_summary
                && let Some(path) = input.cleanse_summary_path.as_ref()
            {
                parts.push(format!("cleanse-fallback={}", file_epoch_secs(path)));
            }
        }
        SourceFamily::Memory => {
            parts.push(format!("memory={}", file_epoch_secs(&paths.memory_file)));
            if let Some(path) = newest_daily_memory_file(paths, cfg)? {
                parts.push(format!(
                    "memory-daily={}:{}",
                    path.display(),
                    file_epoch_secs(&path)
                ));
            }
        }
        SourceFamily::Library => {
            if let Some(path) = newest_library_doc(paths)? {
                parts.push(format!(
                    "library={}:{}",
                    path.display(),
                    file_epoch_secs(&path)
                ));
            }
            parts.push(format!(
                "qmd-lib={}",
                state.last_embed_trigger_epoch_secs.unwrap_or(0)
            ));
        }
        SourceFamily::Distill => {
            if let Some((path, epoch)) = newest_distill_doc(state, cfg) {
                parts.push(format!("distill={}:{}", path.display(), epoch));
            }
            if let Some(path) = newest_daily_memory_file(paths, cfg)? {
                parts.push(format!(
                    "memory-daily={}:{}",
                    path.display(),
                    file_epoch_secs(&path)
                ));
            }
        }
        SourceFamily::Semantic => {
            parts.push(format!(
                "qmd-hot={}",
                state.last_embed_trigger_epoch_secs.unwrap_or(0)
            ));
            parts.push(format!(
                "qmd-lib={}",
                state.last_embed_trigger_epoch_secs.unwrap_or(0)
            ));
        }
    }

    Ok(parts.join("::"))
}

fn route_source_family(query: &str, projection: &ProjectionData) -> SourceFamily {
    let latest_users = latest_goal_lines(projection, 2).join(" ");
    let combined = format!("{query} {latest_users}").to_ascii_lowercase();

    if looks_memory_query(&combined) {
        return SourceFamily::Memory;
    }
    if looks_library_query(&combined) {
        return SourceFamily::Library;
    }
    if looks_distill_query(&combined) {
        return SourceFamily::Distill;
    }
    if looks_semantic_query(&combined) {
        return SourceFamily::Semantic;
    }
    SourceFamily::Hot
}

fn looks_memory_query(lower: &str) -> bool {
    [
        "remember",
        "recall",
        "preference",
        "prefer",
        "decision",
        "decided",
        "agreed",
        "rule",
        "convention",
        "history",
        "why did",
        "user likes",
    ]
    .iter()
    .any(|term| lower.contains(term))
}

fn looks_library_query(lower: &str) -> bool {
    [
        "file", "files", "readme", "document", "docs", "contract", "spec", "api", "module",
        "function", "codebase", "source", ".rs", ".ts", ".js", "where is",
    ]
    .iter()
    .any(|term| lower.contains(term))
}

fn looks_distill_query(lower: &str) -> bool {
    [
        "earlier",
        "previous work",
        "prior work",
        "historical",
        "archive",
        "distill",
        "synthesis",
        "last time",
        "previous rollout",
        "retrospective",
        "postmortem",
    ]
    .iter()
    .any(|term| lower.contains(term))
}

fn looks_semantic_query(lower: &str) -> bool {
    ["similar", "related", "analogous", "closest example"]
        .iter()
        .any(|term| lower.contains(term))
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

fn recent_activity_lines(
    projection: &ProjectionData,
    query_terms: &[String],
    limit: usize,
) -> Vec<String> {
    projection
        .entries
        .iter()
        .rev()
        .filter(|entry| entry.role != "user")
        .filter_map(render_projection_candidate)
        .filter(|line| candidate_is_relevant("hot", SourceFamily::Hot, line, query_terms))
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

fn newest_daily_memory_file(
    paths: &MoonPaths,
    cfg: &MoonContextPacketConfig,
) -> Result<Option<PathBuf>> {
    Ok(
        recent_markdown_files(&paths.memory_dir, cfg.recent_memory_files)?
            .into_iter()
            .find(|path| path != &paths.memory_file),
    )
}

fn newest_library_doc(paths: &MoonPaths) -> Result<Option<PathBuf>> {
    Ok(recent_markdown_files(&paths.mlib_dir, 1)?
        .into_iter()
        .next())
}

fn newest_distill_doc(state: &MoonState, cfg: &MoonContextPacketConfig) -> Option<(PathBuf, u64)> {
    recent_distill_paths(state, cfg.recent_distill_docs)
        .into_iter()
        .next()
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

fn select_doc_candidates(body: &str, query_terms: &[String], limit: usize) -> Vec<SelectedDocLine> {
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
            SelectedDocLine {
                text: cleaned,
                score,
            }
        })
        .collect::<Vec<_>>();
    scored.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.text.cmp(&right.text))
    });
    let mut out = scored
        .iter()
        .filter(|line| line.score > 0)
        .take(limit)
        .cloned()
        .collect::<Vec<_>>();
    out.retain(|line| !line.text.trim().is_empty());
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
    append_section(
        &mut out,
        "Context Coverage",
        &[format!(
            "decision={} reason={}",
            sections.coverage.decision, sections.coverage.reason
        )],
    );
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

fn score_text(kind: &str, text: &str, query_terms: &[String], primary_family: SourceFamily) -> i64 {
    let base = match kind {
        "cleanse" => 85,
        "memory-file" => 80,
        "memory-daily" => 72,
        "library" => 70,
        "distill" => 68,
        "qmd" => 60,
        "hot" => 58,
        _ => 40,
    };
    let lane_bonus = match (primary_family, kind) {
        (SourceFamily::Memory, "memory-file") => 18,
        (SourceFamily::Memory, "memory-daily") => 12,
        (SourceFamily::Library, "library") => 18,
        (SourceFamily::Library, "qmd") => 8,
        (SourceFamily::Distill, "distill") => 18,
        (SourceFamily::Distill, "memory-daily") => 8,
        (SourceFamily::Hot, "hot") => 18,
        (SourceFamily::Hot, "cleanse") => 8,
        (SourceFamily::Semantic, "qmd") => 18,
        _ => 0,
    };
    i64::from(
        base + lane_bonus
            + overlap_score(text, query_terms) * 5
            + if looks_actionable(text) { 10 } else { 0 },
    )
}

fn candidate_is_relevant(
    kind: &str,
    primary_family: SourceFamily,
    text: &str,
    query_terms: &[String],
) -> bool {
    if query_terms.is_empty() {
        return false;
    }
    if overlap_score(text, query_terms) > 0 {
        return true;
    }
    if kind == "hot" && looks_actionable(text) {
        return true;
    }
    kind == "qmd" && primary_family == SourceFamily::Semantic
}

fn evaluate_coverage(
    primary_family: SourceFamily,
    fallback_source: Option<&str>,
    candidate_count: usize,
    top_score: i64,
) -> CoverageReport {
    if candidate_count > 0 {
        let via = fallback_source.unwrap_or("primary");
        return CoverageReport {
            decision: "enough",
            reason: format!(
                "primary={} source={} candidates={} top_score={}",
                primary_family.as_str(),
                via,
                candidate_count,
                top_score
            ),
        };
    }

    let decision = match primary_family {
        SourceFamily::Hot => "current_only",
        SourceFamily::Memory
        | SourceFamily::Library
        | SourceFamily::Distill
        | SourceFamily::Semantic => "search_more",
    };
    CoverageReport {
        decision,
        reason: format!(
            "primary={} candidates=0 top_score=0",
            primary_family.as_str()
        ),
    }
}

fn candidate_source_preference(primary_family: SourceFamily, kind: &str) -> i32 {
    match (primary_family, kind) {
        (SourceFamily::Memory, "memory-file") => 6,
        (SourceFamily::Memory, "memory-daily") => 5,
        (SourceFamily::Library, "library") => 6,
        (SourceFamily::Library, "qmd") => 5,
        (SourceFamily::Distill, "distill") => 6,
        (SourceFamily::Distill, "memory-daily") => 5,
        (SourceFamily::Hot, "hot") => 6,
        (SourceFamily::Hot, "cleanse") => 5,
        (SourceFamily::Semantic, "qmd") => 6,
        (_, "memory-file") => 4,
        (_, "memory-daily") => 3,
        (_, "library") => 3,
        (_, "distill") => 3,
        (_, "cleanse") => 3,
        (_, "hot") => 2,
        (_, "qmd") => 1,
        _ => 0,
    }
}

fn candidate_is_better(
    primary_family: SourceFamily,
    left: &PacketCandidate,
    right: &PacketCandidate,
) -> bool {
    left.score > right.score
        || (left.score == right.score
            && candidate_source_preference(primary_family, left.source_kind)
                > candidate_source_preference(primary_family, right.source_kind))
        || (left.score == right.score
            && candidate_source_preference(primary_family, left.source_kind)
                == candidate_source_preference(primary_family, right.source_kind)
            && left.source_label < right.source_label)
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
        assert_eq!(output.primary_source_family, "hot");
    }

    #[test]
    fn context_packet_routes_memory_queries_to_memory_before_other_sources() {
        let tmp = tempdir().expect("tempdir");
        let paths = test_paths(tmp.path());
        fs::create_dir_all(&paths.raw_dir).expect("mkdir raw");
        fs::create_dir_all(&paths.memory_dir).expect("mkdir memory");
        fs::create_dir_all(&paths.mlib_dir).expect("mkdir mlib");

        fs::write(
            paths.raw_dir.join("s2.jsonl"),
            format!(
                "{}\n{}\n",
                json!({"message":{"role":"user","content":[{"type":"text","text":"Remember the user's release note preference."}]}}),
                json!({"message":{"role":"assistant","content":[{"type":"text","text":"I will check memory first."}]}})
            ),
        )
        .expect("write raw");
        fs::write(
            &paths.memory_file,
            "# MEMORY\n- Preference: release notes should stay concise and factual.\n",
        )
        .expect("write memory");
        fs::write(
            paths.memory_dir.join("2026-04-22.md"),
            "# Daily Memory\n- Preference: older duplicate line.\n",
        )
        .expect("write daily memory");
        fs::write(
            paths.mlib_dir.join("README.md"),
            "# README\n- Library note: plugin config lives in assets/plugin.\n",
        )
        .expect("write library");

        let output = build_context_packet(
            &paths,
            &MoonState::default(),
            &MoonContextPacketConfig::default(),
            &ContextPacketInput {
                session_id: "s2".to_string(),
                raw_source_path: paths.raw_dir.join("s2.jsonl"),
                cleanse_summary_path: None,
                replay_has_compaction_summary: false,
            },
        )
        .expect("build packet");

        assert_eq!(output.primary_source_family, "memory");
        assert_eq!(output.fallback_source, None);
        assert!(
            output
                .content
                .contains("[memory] Preference: release notes should stay concise and factual.")
        );
        assert!(!output.content.contains("[lib:README]"));
    }

    #[test]
    fn context_packet_routes_library_queries_without_memory_fanout() {
        let tmp = tempdir().expect("tempdir");
        let paths = test_paths(tmp.path());
        fs::create_dir_all(&paths.raw_dir).expect("mkdir raw");
        fs::create_dir_all(&paths.memory_dir).expect("mkdir memory");
        fs::create_dir_all(&paths.mlib_dir).expect("mkdir mlib");

        fs::write(
            paths.raw_dir.join("s3.jsonl"),
            format!(
                "{}\n{}\n",
                json!({"message":{"role":"user","content":[{"type":"text","text":"Which file documents the plugin config schema?"}]}}),
                json!({"message":{"role":"assistant","content":[{"type":"text","text":"I will look in the library docs."}]}})
            ),
        )
        .expect("write raw");
        fs::write(
            &paths.memory_file,
            "# MEMORY\n- Preference: do not use memory for workspace file lookups.\n",
        )
        .expect("write memory");
        fs::write(
            paths.mlib_dir.join("plugin.md"),
            "# Plugin Docs\n- Config schema is documented in assets/plugin/openclaw.plugin.json.\n",
        )
        .expect("write library");

        let output = build_context_packet(
            &paths,
            &MoonState::default(),
            &MoonContextPacketConfig::default(),
            &ContextPacketInput {
                session_id: "s3".to_string(),
                raw_source_path: paths.raw_dir.join("s3.jsonl"),
                cleanse_summary_path: None,
                replay_has_compaction_summary: false,
            },
        )
        .expect("build packet");

        assert_eq!(output.primary_source_family, "library");
        assert!(output.content.contains(
            "[lib:plugin] Config schema is documented in assets/plugin/openclaw.plugin.json."
        ));
        assert!(!output.content.contains("[memory]"));
    }

    #[test]
    fn context_packet_omits_irrelevant_recent_activity() {
        let tmp = tempdir().expect("tempdir");
        let paths = test_paths(tmp.path());
        fs::create_dir_all(&paths.raw_dir).expect("mkdir raw");

        fs::write(
            paths.raw_dir.join("s4.jsonl"),
            format!(
                "{}\n{}\n",
                json!({"message":{"role":"user","content":[{"type":"text","text":"Implement the context packet quality gate."}]}}),
                json!({"message":{"role":"assistant","content":[{"type":"text","text":"The unrelated calendar summary discussed lunch scheduling."}]}})
            ),
        )
        .expect("write raw");

        let output = build_context_packet(
            &paths,
            &MoonState::default(),
            &MoonContextPacketConfig::default(),
            &ContextPacketInput {
                session_id: "s4".to_string(),
                raw_source_path: paths.raw_dir.join("s4.jsonl"),
                cleanse_summary_path: None,
                replay_has_compaction_summary: false,
            },
        )
        .expect("build packet");

        assert_eq!(output.primary_source_family, "hot");
        assert_eq!(output.candidate_count, 0);
        assert_eq!(output.coverage_decision, "current_only");
        assert!(output.content.contains("decision=current_only"));
        assert!(!output.content.contains("lunch scheduling"));
    }

    #[test]
    fn context_packet_marks_memory_miss_as_search_more_without_junk_evidence() {
        let tmp = tempdir().expect("tempdir");
        let paths = test_paths(tmp.path());
        fs::create_dir_all(&paths.raw_dir).expect("mkdir raw");
        fs::create_dir_all(&paths.memory_dir).expect("mkdir memory");

        fs::write(
            paths.raw_dir.join("s5.jsonl"),
            format!(
                "{}\n",
                json!({"message":{"role":"user","content":[{"type":"text","text":"Recall the release cadence preference."}]}})
            ),
        )
        .expect("write raw");
        fs::write(
            &paths.memory_file,
            "# MEMORY\n- Favorite color is blue.\n- Preferred editor theme is light.\n",
        )
        .expect("write memory");

        let output = build_context_packet(
            &paths,
            &MoonState::default(),
            &MoonContextPacketConfig::default(),
            &ContextPacketInput {
                session_id: "s5".to_string(),
                raw_source_path: paths.raw_dir.join("s5.jsonl"),
                cleanse_summary_path: None,
                replay_has_compaction_summary: false,
            },
        )
        .expect("build packet");

        assert_eq!(output.primary_source_family, "memory");
        assert_eq!(output.candidate_count, 0);
        assert_eq!(output.coverage_decision, "search_more");
        assert!(output.content.contains("decision=search_more"));
        assert!(!output.content.contains("Favorite color"));
        assert!(!output.content.contains("editor theme"));
    }
}
