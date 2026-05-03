use anyhow::{Context, Result};
use chrono::{Duration, LocalResult, NaiveDate, TimeZone};
use chrono_tz::Tz;
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
const PACKET_GENERATION_VERSION: u8 = 2;

#[derive(Debug, Clone)]
pub struct ContextPacketInput {
    pub session_id: String,
    pub raw_source_path: PathBuf,
    pub cleanse_summary_path: Option<PathBuf>,
    pub replay_has_compaction_summary: bool,
    pub residential_timezone: String,
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

#[derive(Debug, Clone, Default)]
struct TemporalQueryHint {
    target_local_day: Option<NaiveDate>,
    target_day_token: Option<String>,
    channel_hint: Option<String>,
    relative_day: Option<RelativeDayHint>,
}

#[derive(Debug, Clone, Copy)]
enum RelativeDayHint {
    Yesterday,
    Today,
    Tomorrow,
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

#[derive(Debug, Clone, Copy)]
struct CandidateQuery<'a> {
    query_terms: &'a [String],
    primary_family: SourceFamily,
    temporal_hint: &'a TemporalQueryHint,
    residential_tz: Tz,
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
    let residential_tz = parse_residential_tz(&input.residential_timezone);
    let temporal_hint = extract_temporal_hint(&query, projection, residential_tz);
    let primary_family = route_source_family(&query, projection);
    let generation =
        build_packet_generation(paths, state, cfg, input, projection, &query, &temporal_hint)?;
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

    let query_terms = query_terms(&query, projection, &temporal_hint);
    let candidate_query = CandidateQuery {
        query_terms: &query_terms,
        primary_family,
        temporal_hint: &temporal_hint,
        residential_tz,
    };
    let build = build_sections(paths, state, cfg, input, projection, &candidate_query)?;
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
    query: &CandidateQuery<'_>,
) -> Result<PacketBuild> {
    let current_goal = latest_goal_lines(projection, 2);
    let active_work = recent_activity_lines(projection, query.query_terms, 5);
    let mut relevant_memory = Vec::new();
    let mut open_items = Vec::new();

    let collector = collect_candidates(paths, state, cfg, input, projection, query)?;
    let fallback_source = collector.fallback_source.clone();
    let source_read_count = collector.source_read_count;
    let qmd_query_count = collector.qmd_query_count;
    let candidates = apply_session_focus(
        apply_primary_filters(
            collector.into_sorted_candidates(),
            query.temporal_hint,
            query.residential_tz,
        ),
        &input.session_id,
        query.temporal_hint,
    );
    let candidate_count = candidates.len();
    let positive_candidate_count = candidate_count;
    let top_score = candidates
        .iter()
        .map(|candidate| candidate.score)
        .max()
        .unwrap_or(0);
    let coverage = evaluate_coverage(
        query.primary_family,
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
        if open_items.len() < 6 && looks_actionable_open_item(&candidate.text) {
            open_items.push(truncate_with_ellipsis(&candidate.text, MAX_DOC_LINE_CHARS));
        }
    }

    if open_items.is_empty() && !has_primary_constraints(query.temporal_hint) {
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
    query: &CandidateQuery<'_>,
) -> Result<CandidateCollector> {
    let mut collector = CandidateCollector::new(query.primary_family);
    let qmd_query = build_qmd_query(projection, query.query_terms);

    match query.primary_family {
        SourceFamily::Hot => {
            let summary = collect_hot_candidates(projection, query, &mut collector);
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
                    FALLBACK_SOURCE_LIMIT,
                    query,
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
                PRIMARY_SOURCE_LIMIT,
                query,
                &mut collector,
            )?;
            if primary.positive_count == 0
                && let Some(target_day) = query.temporal_hint.target_local_day
            {
                let target_daily_path = paths
                    .memory_dir
                    .join(format!("{}.md", target_day.format("%Y-%m-%d")));
                if target_daily_path.is_file() && target_daily_path != paths.memory_file {
                    let label = short_source_label("memory", &target_daily_path);
                    let targeted = collect_markdown_candidates_from_path(
                        &target_daily_path,
                        "memory-daily",
                        &label,
                        PRIMARY_SOURCE_LIMIT,
                        query,
                        &mut collector,
                    )?;
                    if targeted.candidate_count > 0 {
                        collector.mark_fallback(label);
                    }
                }
            }
            if primary.positive_count == 0
                && let Some(path) = newest_daily_memory_file(paths, cfg)?
            {
                let label = short_source_label("memory", &path);
                let fallback = collect_markdown_candidates_from_path(
                    &path,
                    "memory-daily",
                    &label,
                    FALLBACK_SOURCE_LIMIT,
                    query,
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
                    PRIMARY_SOURCE_LIMIT,
                    query,
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
                    query,
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
                    PRIMARY_SOURCE_LIMIT,
                    query,
                    &mut collector,
                )?
            } else {
                SourceCollectionSummary::empty()
            };
            if primary.positive_count == 0
                && let Some(target_day) = query.temporal_hint.target_local_day
            {
                let target_daily_path = paths
                    .memory_dir
                    .join(format!("{}.md", target_day.format("%Y-%m-%d")));
                if target_daily_path.is_file() && target_daily_path != paths.memory_file {
                    let label = short_source_label("memory", &target_daily_path);
                    let targeted = collect_markdown_candidates_from_path(
                        &target_daily_path,
                        "memory-daily",
                        &label,
                        FALLBACK_SOURCE_LIMIT,
                        query,
                        &mut collector,
                    )?;
                    if targeted.candidate_count > 0 {
                        collector.mark_fallback(label);
                    }
                }
            }
            if primary.positive_count == 0
                && let Some(path) = newest_daily_memory_file(paths, cfg)?
            {
                let label = short_source_label("memory", &path);
                let fallback = collect_markdown_candidates_from_path(
                    &path,
                    "memory-daily",
                    &label,
                    FALLBACK_SOURCE_LIMIT,
                    query,
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
                query,
                &mut collector,
            );
            if hot.positive_count == 0 && cfg.qmd_limit > 0 {
                let lib = collect_qmd_candidates(
                    paths,
                    LIBRARY_EMBED_COLLECTION,
                    "qmd-lib",
                    &qmd_query,
                    cfg.qmd_limit,
                    query,
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
    query: &CandidateQuery<'_>,
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
        if !candidate_is_relevant(
            "hot",
            "hot",
            collector.primary_family,
            &text,
            query.query_terms,
            query.temporal_hint,
            query.residential_tz,
        ) {
            continue;
        }
        let score = score_text(
            "hot",
            "hot",
            &text,
            query.query_terms,
            collector.primary_family,
            query.temporal_hint,
            query.residential_tz,
        );
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
    limit: usize,
    query: &CandidateQuery<'_>,
    collector: &mut CandidateCollector,
) -> Result<SourceCollectionSummary> {
    if !path.is_file() {
        return Ok(SourceCollectionSummary::empty());
    }
    collector.note_file_read();
    let body = read_markdown_body(path)?;
    let selected = select_doc_candidates(
        &body,
        query.query_terms,
        limit,
        source_label,
        query.temporal_hint,
        query.residential_tz,
    );
    let mut summary = SourceCollectionSummary::empty();
    for line in selected {
        if !candidate_is_relevant(
            source_kind,
            source_label,
            collector.primary_family,
            &line.text,
            query.query_terms,
            query.temporal_hint,
            query.residential_tz,
        ) {
            continue;
        }
        let score = score_text(
            source_kind,
            source_label,
            &line.text,
            query.query_terms,
            collector.primary_family,
            query.temporal_hint,
            query.residential_tz,
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
    candidate_query: &CandidateQuery<'_>,
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
        let resolved_label = if hit.source_label.is_empty() {
            source_label.to_string()
        } else {
            format!("{source_label}:{}", hit.source_label)
        };
        if !candidate_is_relevant(
            "qmd",
            &resolved_label,
            collector.primary_family,
            &hit.text,
            candidate_query.query_terms,
            candidate_query.temporal_hint,
            candidate_query.residential_tz,
        ) {
            continue;
        }
        let score = score_text(
            "qmd",
            &resolved_label,
            &hit.text,
            candidate_query.query_terms,
            collector.primary_family,
            candidate_query.temporal_hint,
            candidate_query.residential_tz,
        );
        summary.note_candidate();
        collector.add_candidate(PacketCandidate {
            source_kind: "qmd",
            source_label: resolved_label,
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
    temporal_hint: &TemporalQueryHint,
) -> Result<String> {
    let primary_family = route_source_family(query, projection);
    let mut parts = vec![
        format!("v={PACKET_GENERATION_VERSION}"),
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
        format!("tz={}", input.residential_timezone),
        format!(
            "target_day={}",
            temporal_hint
                .target_day_token
                .clone()
                .unwrap_or_else(|| "none".to_string())
        ),
        format!(
            "target_channel={}",
            temporal_hint
                .channel_hint
                .clone()
                .unwrap_or_else(|| "none".to_string())
        ),
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

fn parse_residential_tz(raw: &str) -> Tz {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        chrono_tz::UTC
    } else {
        trimmed.parse::<Tz>().unwrap_or(chrono_tz::UTC)
    }
}

fn extract_temporal_hint(
    query: &str,
    projection: &ProjectionData,
    residential_tz: Tz,
) -> TemporalQueryHint {
    let anchor_local_day = projection
        .time_end_epoch
        .and_then(|epoch| local_day_for_epoch(epoch, residential_tz))
        .or_else(|| {
            projection
                .time_start_epoch
                .and_then(|epoch| local_day_for_epoch(epoch, residential_tz))
        })
        .or_else(|| {
            Some(
                residential_tz
                    .from_utc_datetime(&chrono::Utc::now().naive_utc())
                    .date_naive(),
            )
        });

    let lower = query.to_ascii_lowercase();
    let explicit_day = extract_iso_date(query);
    let relative_day = if lower.contains("yesterday") || lower.contains("last night") {
        Some(RelativeDayHint::Yesterday)
    } else if lower.contains("today") {
        Some(RelativeDayHint::Today)
    } else if lower.contains("tomorrow") {
        Some(RelativeDayHint::Tomorrow)
    } else {
        None
    };
    let relative_day_target = match relative_day {
        Some(RelativeDayHint::Yesterday) => anchor_local_day.map(|day| day - Duration::days(1)),
        Some(RelativeDayHint::Today) => anchor_local_day,
        Some(RelativeDayHint::Tomorrow) => anchor_local_day.map(|day| day + Duration::days(1)),
        None => None,
    };
    let target_local_day = explicit_day.or(relative_day_target);
    let target_day_token = target_local_day.map(|day| day.format("%Y-%m-%d").to_string());
    let channel_hint = extract_discord_channel(query).or_else(|| {
        projection
            .entries
            .iter()
            .rev()
            .take(8)
            .find_map(|entry| extract_discord_channel(&entry.content))
    });

    TemporalQueryHint {
        target_local_day,
        target_day_token,
        channel_hint,
        relative_day,
    }
}

fn local_day_for_epoch(epoch_secs: u64, residential_tz: Tz) -> Option<NaiveDate> {
    match residential_tz.timestamp_opt(epoch_secs as i64, 0) {
        LocalResult::Single(dt) => Some(dt.date_naive()),
        _ => None,
    }
}

fn extract_iso_date(raw: &str) -> Option<NaiveDate> {
    raw.split_whitespace()
        .map(|token| {
            token
                .trim_matches(|ch: char| !ch.is_ascii_digit() && ch != '-')
                .to_string()
        })
        .find_map(|token| {
            if token.len() != 10 {
                return None;
            }
            if !matches!(token.as_bytes().get(4), Some(b'-'))
                || !matches!(token.as_bytes().get(7), Some(b'-'))
            {
                return None;
            }
            NaiveDate::parse_from_str(&token, "%Y-%m-%d").ok()
        })
}

fn extract_discord_channel(raw: &str) -> Option<String> {
    if let Some(value) = extract_token_after(raw, "channel=") {
        return Some(value);
    }
    if let Some(value) = extract_token_after(raw, "channel_id=") {
        return Some(value);
    }
    if let Some(value) = extract_token_after(raw, "channel id:") {
        return Some(value);
    }

    let lower = raw.to_ascii_lowercase();
    let marker = "\"group_channel\"";
    let idx = lower.find(marker)?;
    let rest = raw.get(idx + marker.len()..)?.trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    let value = rest.get(..end)?.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn extract_token_after(raw: &str, marker: &str) -> Option<String> {
    let lower = raw.to_ascii_lowercase();
    let start = lower.find(&marker.to_ascii_lowercase())? + marker.len();
    let rest = raw.get(start..)?.trim_start();
    let token = rest
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '#' | '_' | '-' | ':'))
        .collect::<String>();
    let token = token
        .trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '#')
        .to_string();
    if token.is_empty() { None } else { Some(token) }
}

fn normalize_channel(raw: &str) -> String {
    let trimmed = raw.trim().to_ascii_lowercase();
    if let Some(stripped) = trimmed.strip_prefix("channel:") {
        stripped.trim().to_string()
    } else {
        trimmed
    }
}

fn parse_candidate_timestamp(text: &str) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    let trimmed = text.trim_start();
    let tail = trimmed.strip_prefix('[')?;
    let end = tail.find(']')?;
    let raw_ts = tail.get(..end)?;
    chrono::DateTime::parse_from_rfc3339(raw_ts).ok()
}

fn candidate_local_day(text: &str, source_label: &str, residential_tz: Tz) -> Option<NaiveDate> {
    parse_candidate_timestamp(text)
        .map(|dt| dt.with_timezone(&residential_tz).date_naive())
        .or_else(|| extract_iso_date(source_label))
        .or_else(|| extract_iso_date(text))
}

fn candidate_channel(text: &str, source_label: &str) -> Option<String> {
    extract_discord_channel(text).or_else(|| extract_discord_channel(source_label))
}

fn temporal_alignment_score(
    source_label: &str,
    text: &str,
    temporal_hint: &TemporalQueryHint,
    residential_tz: Tz,
) -> i64 {
    let Some(target_day) = temporal_hint.target_local_day else {
        return 0;
    };
    let Some(candidate_day) = candidate_local_day(text, source_label, residential_tz) else {
        return 0;
    };
    let delta_days = (candidate_day - target_day).num_days().abs();
    if delta_days > 0 && line_mentions_relative_day(text, temporal_hint.relative_day) {
        return 10;
    }
    match delta_days {
        0 => 24,
        1 => 8,
        2 => 2,
        _ => -18,
    }
}

fn channel_alignment_score(
    source_label: &str,
    text: &str,
    temporal_hint: &TemporalQueryHint,
) -> i64 {
    let Some(target_channel) = temporal_hint.channel_hint.as_ref() else {
        return 0;
    };
    let Some(candidate) = candidate_channel(text, source_label) else {
        return 0;
    };
    if normalize_channel(&candidate) == normalize_channel(target_channel) {
        18
    } else {
        -30
    }
}

fn recap_penalty(text: &str) -> i64 {
    let lower = text.to_ascii_lowercase();
    let mut penalty = 0i64;
    for phrase in [
        "the original message was",
        "the original line was",
        "original msg was",
        "[[reply_to_current]]",
        "i can also reconstruct the surrounding wording",
    ] {
        if lower.contains(phrase) {
            penalty += 12;
        }
    }
    -penalty
}

fn line_mentions_relative_day(text: &str, relative_day: Option<RelativeDayHint>) -> bool {
    let Some(relative_day) = relative_day else {
        return false;
    };
    let lower = text.to_ascii_lowercase();
    match relative_day {
        RelativeDayHint::Yesterday => lower.contains("yesterday") || lower.contains("last night"),
        RelativeDayHint::Today => lower.contains("today"),
        RelativeDayHint::Tomorrow => lower.contains("tomorrow"),
    }
}

fn query_terms(
    query: &str,
    projection: &ProjectionData,
    temporal_hint: &TemporalQueryHint,
) -> Vec<String> {
    let mut out = tokenize(query);
    let has_current_query = !query.trim().is_empty();
    if let Some(target_day) = temporal_hint.target_day_token.as_ref() {
        out.push(target_day.clone());
    }
    if let Some(channel) = temporal_hint.channel_hint.as_ref() {
        let normalized = normalize_channel(channel);
        out.extend(tokenize(&normalized));
    }
    if out.len() < 4 {
        if has_current_query {
            out.extend(recent_projection_terms(projection, 6));
        } else {
            out.extend(projection.keywords.iter().flat_map(|value| tokenize(value)));
        }
    }
    out.sort();
    out.dedup();
    out
}

fn recent_projection_terms(projection: &ProjectionData, limit: usize) -> Vec<String> {
    projection
        .entries
        .iter()
        .rev()
        .filter(|entry| entry.role == "user" || entry.role == "assistant")
        .take(limit)
        .flat_map(|entry| tokenize(&entry.content))
        .collect()
}

fn tokenize(raw: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut ascii = String::new();
    let mut cjk = String::new();
    let mut unicode = String::new();

    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            flush_cjk_terms(&mut cjk, &mut terms);
            flush_unicode_term(&mut unicode, &mut terms);
            ascii.push(ch.to_ascii_lowercase());
        } else if is_cjk_char(ch) {
            flush_ascii_term(&mut ascii, &mut terms);
            flush_unicode_term(&mut unicode, &mut terms);
            cjk.push(ch);
        } else if ch.is_alphanumeric() {
            flush_ascii_term(&mut ascii, &mut terms);
            flush_cjk_terms(&mut cjk, &mut terms);
            unicode.extend(ch.to_lowercase());
        } else {
            flush_ascii_term(&mut ascii, &mut terms);
            flush_cjk_terms(&mut cjk, &mut terms);
            flush_unicode_term(&mut unicode, &mut terms);
        }
    }

    flush_ascii_term(&mut ascii, &mut terms);
    flush_cjk_terms(&mut cjk, &mut terms);
    flush_unicode_term(&mut unicode, &mut terms);
    terms
}

fn flush_ascii_term(token: &mut String, terms: &mut Vec<String>) {
    if token.len() >= 3 && !is_ascii_stopword(token) {
        terms.push(std::mem::take(token));
    } else {
        token.clear();
    }
}

fn flush_unicode_term(token: &mut String, terms: &mut Vec<String>) {
    if token.chars().count() >= 2 {
        terms.push(std::mem::take(token));
    } else {
        token.clear();
    }
}

fn flush_cjk_terms(token: &mut String, terms: &mut Vec<String>) {
    let chars = token.chars().collect::<Vec<_>>();
    match chars.len() {
        0 => {}
        1 => {
            if !is_cjk_stop_char(chars[0]) {
                terms.push(chars[0].to_string());
            }
        }
        _ => {
            for pair in chars.windows(2) {
                terms.push(pair.iter().collect());
            }
            for ch in chars {
                if !is_cjk_stop_char(ch) {
                    terms.push(ch.to_string());
                }
            }
        }
    }
    token.clear();
}

fn is_ascii_stopword(token: &str) -> bool {
    matches!(
        token,
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
}

fn is_cjk_char(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xF900..=0xFAFF
            | 0x3040..=0x30FF
            | 0xAC00..=0xD7AF
    )
}

fn is_cjk_stop_char(ch: char) -> bool {
    matches!(
        ch,
        '的' | '了'
            | '是'
            | '在'
            | '有'
            | '和'
            | '或'
            | '但'
            | '也'
            | '就'
            | '都'
            | '很'
            | '还'
            | '没'
            | '不'
            | '我'
            | '你'
            | '他'
            | '她'
            | '它'
            | '们'
            | '这'
            | '那'
            | '个'
            | '一'
            | '上'
            | '下'
            | '到'
            | '请'
            | '吗'
            | '呢'
            | '啊'
            | '吧'
    )
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
        .filter(|line| {
            candidate_is_relevant(
                "hot",
                "hot",
                SourceFamily::Hot,
                line,
                query_terms,
                &TemporalQueryHint::default(),
                chrono_tz::UTC,
            )
        })
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

fn select_doc_candidates(
    body: &str,
    query_terms: &[String],
    limit: usize,
    source_label: &str,
    temporal_hint: &TemporalQueryHint,
    residential_tz: Tz,
) -> Vec<SelectedDocLine> {
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
                + if cleaned.starts_with('#') { 1 } else { 0 }
                + i32::try_from(temporal_alignment_score(
                    source_label,
                    &cleaned,
                    temporal_hint,
                    residential_tz,
                ))
                .unwrap_or(0)
                + i32::try_from(channel_alignment_score(
                    source_label,
                    &cleaned,
                    temporal_hint,
                ))
                .unwrap_or(0)
                + i32::try_from(recap_penalty(&cleaned)).unwrap_or(0);
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
    truncate_packet_text(out.trim_end(), max_chars)
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

fn truncate_packet_text(input: &str, max_chars: usize) -> String {
    let clean: String = input
        .chars()
        .filter(|ch| !ch.is_control() || matches!(ch, '\n' | '\t'))
        .collect();
    if clean.chars().count() > max_chars {
        let mut s: String = clean.chars().take(max_chars).collect();
        s.push('…');
        s
    } else {
        clean
    }
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

fn score_text(
    kind: &str,
    source_label: &str,
    text: &str,
    query_terms: &[String],
    primary_family: SourceFamily,
    temporal_hint: &TemporalQueryHint,
    residential_tz: Tz,
) -> i64 {
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
    let mut score = i64::from(
        base + lane_bonus
            + overlap_score(text, query_terms) * 5
            + if looks_actionable(text) { 10 } else { 0 },
    );
    score += temporal_alignment_score(source_label, text, temporal_hint, residential_tz);
    score += channel_alignment_score(source_label, text, temporal_hint);
    score += recap_penalty(text);
    if text.to_ascii_lowercase().contains("[discord ") {
        score += 6;
    }
    score
}

fn candidate_is_relevant(
    kind: &str,
    source_label: &str,
    primary_family: SourceFamily,
    text: &str,
    query_terms: &[String],
    temporal_hint: &TemporalQueryHint,
    residential_tz: Tz,
) -> bool {
    if has_primary_constraints(temporal_hint)
        && candidate_matches_primary(text, source_label, temporal_hint, residential_tz)
    {
        return true;
    }
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

fn looks_actionable_open_item(text: &str) -> bool {
    if !looks_actionable(text) {
        return false;
    }
    let lower = text.to_ascii_lowercase();
    !(lower.contains("providers declare their own auth/readiness")
        || lower.contains("use action=\"list\"")
        || lower.contains("generated images are delivered automatically")
        || lower.contains("[tool-input]"))
}

fn has_primary_constraints(temporal_hint: &TemporalQueryHint) -> bool {
    temporal_hint.target_local_day.is_some() || temporal_hint.channel_hint.is_some()
}

fn apply_primary_filters(
    candidates: Vec<PacketCandidate>,
    temporal_hint: &TemporalQueryHint,
    residential_tz: Tz,
) -> Vec<PacketCandidate> {
    if !has_primary_constraints(temporal_hint) {
        return candidates;
    }

    let primary = candidates
        .iter()
        .filter(|candidate| {
            candidate_matches_primary(
                &candidate.text,
                &candidate.source_label,
                temporal_hint,
                residential_tz,
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    if primary.is_empty() {
        candidates
    } else {
        primary
    }
}

fn apply_session_focus(
    candidates: Vec<PacketCandidate>,
    session_id: &str,
    temporal_hint: &TemporalQueryHint,
) -> Vec<PacketCandidate> {
    if session_id.trim().is_empty() || !has_primary_constraints(temporal_hint) {
        return candidates;
    }

    let needle = session_id.trim().to_ascii_lowercase();
    let focused = candidates
        .iter()
        .filter(|candidate| {
            matches!(
                candidate.source_kind,
                "hot" | "memory-file" | "memory-daily" | "cleanse"
            ) || candidate
                .source_label
                .to_ascii_lowercase()
                .contains(&needle)
        })
        .cloned()
        .collect::<Vec<_>>();
    if focused.is_empty() {
        candidates
    } else {
        focused
    }
}

fn candidate_matches_primary(
    text: &str,
    source_label: &str,
    temporal_hint: &TemporalQueryHint,
    residential_tz: Tz,
) -> bool {
    if let Some(target_day) = temporal_hint.target_local_day {
        let Some(candidate_day) = candidate_local_day(text, source_label, residential_tz) else {
            return false;
        };
        if candidate_day != target_day
            && !line_mentions_relative_day(text, temporal_hint.relative_day)
        {
            return false;
        }
    }

    if let Some(target_channel) = temporal_hint.channel_hint.as_ref() {
        let Some(candidate_channel) = candidate_channel(text, source_label) else {
            return false;
        };
        if normalize_channel(&candidate_channel) != normalize_channel(target_channel) {
            return false;
        }
    }

    true
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
                residential_timezone: "UTC".to_string(),
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
                residential_timezone: "UTC".to_string(),
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
                residential_timezone: "UTC".to_string(),
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
                residential_timezone: "UTC".to_string(),
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
    fn context_packet_keeps_chinese_topic_switch_from_reusing_stale_english_terms() {
        let tmp = tempdir().expect("tempdir");
        let paths = test_paths(tmp.path());
        fs::create_dir_all(&paths.raw_dir).expect("mkdir raw");

        fs::write(
            paths.raw_dir.join("s-topic-switch.jsonl"),
            format!(
                "{}\n{}\n{}\n{}\n{}\n{}\n{}\n",
                json!({"message":{"role":"user","content":[{"type":"text","text":"Can you set commands.native=true and restart the gateway?"}]}}),
                json!({"message":{"role":"assistant","content":[{"type":"text","text":"I will inspect commands.native and the Discord gateway status."}]}}),
                json!({"message":{"role":"user","content":[{"type":"text","text":"JJ鼻涕白，打喷嚏。晚上鼻塞，白天也打喷嚏。"}]}}),
                json!({"message":{"role":"assistant","content":[{"type":"text","text":"晚上可以先做白萝卜雪梨汤，清淡补水。"}]}}),
                json!({"message":{"role":"user","content":[{"type":"text","text":"我们在讨论汤，请不要打岔到吃药上。"}]}}),
                json!({"message":{"role":"assistant","content":[{"type":"text","text":"收到，我们只聊汤，不扯吃药。"}]}}),
                json!({"message":{"role":"user","content":[{"type":"text","text":"请继续"}]}})
            ),
        )
        .expect("write raw");

        let output = build_context_packet(
            &paths,
            &MoonState::default(),
            &MoonContextPacketConfig::default(),
            &ContextPacketInput {
                session_id: "s-topic-switch".to_string(),
                raw_source_path: paths.raw_dir.join("s-topic-switch.jsonl"),
                cleanse_summary_path: None,
                replay_has_compaction_summary: false,
                residential_timezone: "Australia/Sydney".to_string(),
            },
        )
        .expect("build packet");

        assert!(output.content.contains("我们在讨论汤"));
        assert!(output.content.contains("我们只聊汤"));
        assert!(!output.content.contains("commands.native"));
        assert!(!output.content.contains("gateway status"));
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
                residential_timezone: "UTC".to_string(),
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
