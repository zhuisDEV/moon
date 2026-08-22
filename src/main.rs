use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use moon::redaction::redact_text;
use moon::{
    ContextRequest, DistillInput, EmbeddingProvider, EvidenceInput, HashEmbedding, IngestDocument,
    LocalEmbedding, MemoryInput, ReviewOutcome, RuntimeMetricInput, SearchMode, SearchRequest,
    Store,
};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Instant, UNIX_EPOCH};
use walkdir::WalkDir;

const MAX_INPUT_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Parser)]
#[command(name = "moon", version, about)]
struct Cli {
    /// Moon runtime root.
    #[arg(long, env = "MOON_HOME")]
    home: Option<PathBuf>,

    /// Override the SQLite database path.
    #[arg(long, env = "MOON_DATABASE")]
    database: Option<PathBuf>,

    /// Fixed vector dimensions for this database.
    #[arg(long, env = "MOON_EMBEDDING_DIMENSIONS", default_value_t = 384)]
    dimensions: usize,

    /// Emit machine-readable JSON.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create or migrate the isolated database.
    Init,
    /// Store one structured canonical memory.
    Remember(RememberArgs),
    /// Record one completed session as immutable, secret-scrubbed evidence.
    Record(RecordArgs),
    /// Distill or confirm one evidence-backed canonical memory.
    Distill(DistillArgs),
    /// Distill a validated batch from one completed evidence session.
    DistillBatch(DistillBatchArgs),
    /// Assemble a bounded, cited memory packet for an agent turn.
    Context(ContextArgs),
    /// Incrementally index a file or directory.
    Ingest(IngestArgs),
    /// Embed queued chunks inside SQLite.
    Embed(EmbedArgs),
    /// Search indexed memory.
    Search(SearchArgs),
    /// Read old Moon files and import them without modifying the source.
    ImportLegacy(ImportLegacyArgs),
    /// Check database integrity, schema, indexes, and queue state.
    Health,
    /// Create a consistent online SQLite backup.
    Backup(BackupArgs),
    /// Export canonical structured memories as generated Markdown.
    Export(ExportArgs),
    /// Compare native results with a read-only lexical scan of old Moon files.
    Shadow(ShadowArgs),
    /// Rebuild the FTS5 index from canonical chunks.
    RebuildFts,
    /// Clear vectors and enqueue all active chunks for re-embedding.
    RequeueEmbeddings,
    /// Read or write JSON runtime state.
    State(StateArgs),
    /// Measure repeated search latency against the current database.
    Benchmark(BenchmarkArgs),
    /// Inspect, review, export, or prune privacy-preserving context metrics.
    Metrics(MetricsArgs),
    /// Keep the local embedding model warm over a private JSON-lines channel.
    Serve(ServeArgs),
    /// Check for or apply a signed compatibility-set update.
    Update(UpdateArgs),
}

#[derive(Debug, Args)]
struct RememberArgs {
    #[arg(long, conflicts_with = "file")]
    content: Option<String>,
    #[arg(long, conflicts_with = "content")]
    file: Option<PathBuf>,
    #[arg(long, default_value = "fact")]
    kind: String,
    #[arg(long, default_value = "global")]
    scope: String,
    #[arg(long)]
    title: Option<String>,
    #[arg(long, default_value_t = 0.5)]
    importance: f64,
    #[arg(long, default_value_t = 1.0)]
    confidence: f64,
    #[arg(long)]
    pinned: bool,
}

#[derive(Debug, Args)]
struct RecordArgs {
    #[arg(long)]
    session_id: String,
    #[arg(long, conflicts_with = "file")]
    content: Option<String>,
    #[arg(long, conflicts_with = "content")]
    file: Option<PathBuf>,
    #[arg(long, default_value = "global")]
    scope: String,
    #[arg(long)]
    title: Option<String>,
    /// Unix timestamp in milliseconds; defaults to the current time.
    #[arg(long)]
    completed_at_ms: Option<i64>,
    #[arg(long, default_value = "{}")]
    metadata_json: String,
}

#[derive(Debug, Args)]
struct DistillArgs {
    #[arg(long)]
    key: String,
    #[arg(long)]
    session_id: String,
    #[arg(long, required_unless_present = "proposal_json")]
    evidence_quote: Option<String>,
    #[arg(long, conflicts_with = "file")]
    content: Option<String>,
    #[arg(long, conflicts_with = "content")]
    file: Option<PathBuf>,
    /// Read `content` and `evidence_quote` as one JSON object from stdin.
    #[arg(
        long,
        conflicts_with_all = ["content", "file", "evidence_quote"]
    )]
    proposal_json: bool,
    #[arg(long, default_value = "fact")]
    kind: String,
    #[arg(long, default_value = "global")]
    scope: String,
    #[arg(long)]
    title: Option<String>,
    #[arg(long, default_value_t = 0.5)]
    importance: f64,
    #[arg(long, default_value_t = 1.0)]
    confidence: f64,
    #[arg(long)]
    pinned: bool,
    /// Active memory document id being deliberately replaced.
    #[arg(long)]
    supersedes: Option<i64>,
}

#[derive(Debug, Args)]
struct DistillBatchArgs {
    #[arg(long)]
    session_id: String,
    #[arg(long, default_value = "global")]
    scope: String,
}

#[derive(Debug, Args)]
struct ContextArgs {
    #[arg(long)]
    query: String,
    #[arg(long, default_value = "hybrid")]
    mode: SearchMode,
    #[arg(long, default_value_t = 8)]
    limit: usize,
    #[arg(long)]
    scope: Option<String>,
    #[arg(long, default_value_t = 3_500)]
    max_chars: usize,
    #[arg(long, default_value_t = 2)]
    evidence_per_memory: usize,
    /// Emit the private adapter envelope; requires --json.
    #[arg(long, hide = true)]
    adapter: bool,
    #[command(flatten)]
    provider: ProviderArgs,
}

#[derive(Debug, Args)]
struct IngestArgs {
    #[arg(long)]
    path: PathBuf,
    #[arg(long, default_value = "library")]
    kind: String,
    #[arg(long, default_value = "global")]
    scope: String,
    #[arg(long)]
    title: Option<String>,
    #[arg(long)]
    recursive: bool,
}

#[derive(Debug, Args, Clone)]
struct ProviderArgs {
    /// Deterministic, local-only vector plumbing provider.
    #[arg(long, default_value = "hash")]
    provider: String,
}

#[derive(Debug, Args, Clone)]
struct EmbedArgs {
    #[command(flatten)]
    provider: ProviderArgs,
    #[arg(long, default_value_t = 64)]
    limit: usize,
    /// Keep the provider warm and drain every currently available job.
    #[arg(long)]
    drain: bool,
}

#[derive(Debug, Args)]
struct SearchArgs {
    #[arg(long)]
    query: String,
    #[arg(long, default_value = "hybrid")]
    mode: SearchMode,
    #[arg(long, default_value_t = 8)]
    limit: usize,
    #[arg(long)]
    scope: Option<String>,
    #[arg(long)]
    kind: Option<String>,
    #[command(flatten)]
    provider: ProviderArgs,
}

#[derive(Debug, Args)]
struct ImportLegacyArgs {
    #[arg(long, default_value = "~/.moon")]
    source_home: String,
    #[arg(long)]
    include_raw: bool,
    #[arg(long)]
    dry_run: bool,
}

#[derive(Debug, Args)]
struct BackupArgs {
    #[arg(long)]
    destination: PathBuf,
}

#[derive(Debug, Args)]
struct ExportArgs {
    #[arg(long)]
    destination: PathBuf,
}

#[derive(Debug, Args)]
struct ShadowArgs {
    #[arg(long)]
    query: String,
    #[arg(long, default_value = "~/.moon")]
    legacy_home: String,
    #[arg(long, default_value_t = 8)]
    limit: usize,
    #[arg(long)]
    scope: Option<String>,
    #[command(flatten)]
    provider: ProviderArgs,
}

#[derive(Debug, Args)]
struct StateArgs {
    #[command(subcommand)]
    command: StateCommand,
}

#[derive(Debug, Subcommand)]
enum StateCommand {
    Get { key: String },
    Set { key: String, value_json: String },
}

#[derive(Debug, Args)]
struct BenchmarkArgs {
    #[arg(long)]
    query: String,
    #[arg(long, default_value = "hybrid")]
    mode: SearchMode,
    #[arg(long, default_value_t = 100)]
    iterations: usize,
    #[arg(long, default_value_t = 8)]
    limit: usize,
    #[command(flatten)]
    provider: ProviderArgs,
}

#[derive(Debug, Args)]
struct MetricsArgs {
    #[command(subcommand)]
    command: MetricsCommand,
}

#[derive(Debug, Subcommand)]
enum MetricsCommand {
    /// Summarize context volume, injection, review outcomes, and latency.
    Summary {
        #[arg(long, default_value = "7d")]
        since: String,
    },
    /// List recent opaque request records without queries or recalled content.
    Recent {
        #[arg(long, default_value = "7d")]
        since: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Attach a human quality label to one opaque context request.
    Review {
        #[arg(long)]
        request: String,
        #[arg(long)]
        outcome: ReviewOutcome,
        #[arg(long)]
        expected_rank: Option<usize>,
    },
    #[command(hide = true)]
    MarkInjection {
        #[arg(long)]
        request: String,
        #[arg(long)]
        injected: bool,
    },
    #[command(hide = true)]
    RecordRuntime {
        #[arg(long)]
        kind: String,
        #[arg(long)]
        status: String,
        #[arg(long)]
        duration_us: u64,
        #[arg(long)]
        evidence_changed: bool,
        #[arg(long)]
        learning_eligible: bool,
        #[arg(long)]
        proposed_memories: Option<usize>,
        #[arg(long)]
        accepted_memories: Option<usize>,
        #[arg(long)]
        compacted: bool,
        #[arg(long)]
        tokens_before: Option<usize>,
        #[arg(long)]
        tokens_after: Option<usize>,
    },
    /// Export redacted numeric records to a new owner-only JSON file.
    Export {
        #[arg(long, default_value = "7d")]
        since: String,
        #[arg(long)]
        destination: PathBuf,
    },
    /// Preview or delete records older than the retention window.
    Prune {
        #[arg(long, default_value = "30d")]
        older_than: String,
        /// Apply the deletion; without this flag Moon only reports the count.
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Args)]
struct ServeArgs {
    #[command(flatten)]
    provider: ProviderArgs,
    /// Read and write one JSON object per line over stdin/stdout.
    #[arg(long, default_value_t = true)]
    stdio: bool,
}

#[derive(Debug, Args)]
struct UpdateArgs {
    /// Inspect the stable channel without changing local state.
    #[arg(long, conflicts_with_all = ["dry_run", "yes", "allow_downgrade"])]
    check: bool,
    /// Select one exact stable release.
    #[arg(long)]
    version: Option<String>,
    /// Verify the release and show the plan without changing production state.
    #[arg(long, conflicts_with_all = ["check", "yes"])]
    dry_run: bool,
    /// Apply without an interactive confirmation prompt.
    #[arg(long)]
    yes: bool,
    /// Permit an explicit downgrade to the selected signed release.
    #[arg(long, requires = "version")]
    allow_downgrade: bool,
}

fn main() {
    let wants_json = env::args_os().any(|argument| argument == "--json");
    let wants_version = env::args_os().any(|argument| argument == "--version" || argument == "-V");
    if wants_json && wants_version {
        match moon::version::VersionInfo::current() {
            Ok(version) => println!(
                "{}",
                serde_json::to_string(&version).expect("version identity is serializable")
            ),
            Err(error) => {
                let safe_message = redact_text(&format!("{error:#}")).value;
                eprintln!(
                    "{}",
                    serde_json::json!({
                        "ok": false,
                        "error": {
                            "code": "version_identity_failed",
                            "message": safe_message,
                        }
                    })
                );
                std::process::exit(1);
            }
        }
        return;
    }
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            use clap::error::ErrorKind;
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) {
                print!("{error}");
                return;
            }
            if wants_json {
                eprintln!(
                    "{}",
                    serde_json::json!({
                        "ok": false,
                        "error": {
                            "code": "invalid_arguments",
                            "message": error.to_string(),
                        }
                    })
                );
            } else {
                let _ = error.print();
            }
            std::process::exit(error.exit_code());
        }
    };
    if let Err(error) = run(cli) {
        let safe_message = redact_text(&format!("{error:#}")).value;
        let code = moon::update::error_code(&error).unwrap_or("operation_failed");
        if wants_json {
            eprintln!(
                "{}",
                serde_json::json!({
                    "ok": false,
                    "error": {
                            "code": code,
                        "message": safe_message,
                    }
                })
            );
        } else {
            eprintln!("moon error: {safe_message}");
        }
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    let home = resolve_home(cli.home.as_deref())?;
    if let Command::Update(args) = &cli.command {
        return run_update(&home, cli.dimensions, cli.json, args);
    }
    let database = cli
        .database
        .clone()
        .unwrap_or_else(|| home.join("state/moon.sqlite"));
    if matches!(&cli.command, Command::Health) {
        let store = Store::open_existing(&database, cli.dimensions)?;
        let report = store.health()?;
        emit(&report, cli.json)?;
        if !report.ok {
            anyhow::bail!("health check failed");
        }
        return Ok(());
    }
    let mut store = Store::open(&database, cli.dimensions)?;

    match cli.command {
        Command::Init => emit(
            &serde_json::json!({
                "ok": true,
                "home": home,
                "database": store.path(),
                "dimensions": store.embedding_dimensions(),
            }),
            cli.json,
        ),
        Command::Remember(args) => {
            let content = read_explicit_content(args.content, args.file.as_deref())?;
            let outcome = store.remember(MemoryInput {
                memory_kind: args.kind,
                scope: args.scope,
                title: args.title,
                content,
                importance: args.importance,
                confidence: args.confidence,
                pinned: args.pinned,
            })?;
            emit(&outcome, cli.json)
        }
        Command::Record(args) => {
            let content = read_explicit_content(args.content, args.file.as_deref())?;
            let outcome = store.record_evidence(EvidenceInput {
                session_id: args.session_id,
                scope: args.scope,
                title: args.title,
                content,
                completed_at_ms: args.completed_at_ms.unwrap_or_else(current_time_ms),
                metadata_json: args.metadata_json,
            })?;
            emit(&outcome, cli.json)
        }
        Command::Distill(args) => {
            let (content, evidence_quote, title) = if args.proposal_json {
                let payload: DistillProposalPayload =
                    serde_json::from_str(&read_bounded_stdin("distillation proposal")?)
                        .context("distillation proposal must be valid JSON")?;
                (
                    payload.content,
                    payload.evidence_quote,
                    payload.title.or(args.title),
                )
            } else {
                (
                    read_explicit_content(args.content, args.file.as_deref())?,
                    args.evidence_quote
                        .context("--evidence-quote is required unless --proposal-json is used")?,
                    args.title,
                )
            };
            let outcome = store.distill_memory(DistillInput {
                canonical_key: args.key,
                memory_kind: args.kind,
                scope: args.scope,
                title,
                content,
                importance: args.importance,
                confidence: args.confidence,
                pinned: args.pinned,
                evidence_session_id: args.session_id,
                evidence_quote,
                supersedes: args.supersedes,
            })?;
            emit(&outcome, cli.json)
        }
        Command::DistillBatch(args) => {
            let payload: DistillBatchPayload =
                serde_json::from_str(&read_bounded_stdin("distillation batch")?)
                    .context("distillation batch must be valid JSON")?;
            let proposals = match payload {
                DistillBatchPayload::Array(proposals) => proposals,
                DistillBatchPayload::Object { memories } => memories,
            };
            if proposals.len() > 32 {
                anyhow::bail!("distillation batch may contain at most 32 proposals");
            }
            let mut outcomes = Vec::with_capacity(proposals.len());
            for proposal in proposals {
                outcomes.push(store.distill_memory(DistillInput {
                    canonical_key: proposal.canonical_key,
                    memory_kind: proposal.kind,
                    scope: args.scope.clone(),
                    title: proposal.title,
                    content: proposal.content,
                    importance: proposal.importance,
                    confidence: proposal.confidence,
                    pinned: proposal.pinned,
                    evidence_session_id: args.session_id.clone(),
                    evidence_quote: proposal.evidence_quote,
                    supersedes: proposal.supersedes_document_id,
                })?);
            }
            emit(
                &serde_json::json!({
                    "distilled": outcomes.len(),
                    "outcomes": outcomes,
                }),
                cli.json,
            )
        }
        Command::Context(args) => {
            let provider = if args.mode == SearchMode::Lexical {
                None
            } else {
                Some(build_provider(&args.provider, cli.dimensions, &home)?)
            };
            let observation = store.observe_context(
                &ContextRequest {
                    query: args.query,
                    mode: args.mode,
                    limit: args.limit,
                    scope: args.scope,
                    max_chars: args.max_chars,
                    evidence_per_memory: args.evidence_per_memory,
                },
                provider.as_deref(),
            )?;
            let packet = observation.packet;
            if args.adapter {
                if !cli.json {
                    anyhow::bail!("--adapter requires --json");
                }
                let rendered = (!packet.is_empty()).then(|| packet.render_markdown());
                emit(
                    &serde_json::json!({
                        "request_id": observation.request_id,
                        "packet": rendered,
                        "memory_count": packet.memories.len(),
                        "reference_count": packet.references.len(),
                        "packet_chars": packet.used_chars,
                        "truncated": packet.truncated,
                    }),
                    true,
                )
            } else if cli.json {
                emit(&packet, true)
            } else if packet.is_empty() {
                Ok(())
            } else {
                print!("{}", packet.render_markdown());
                Ok(())
            }
        }
        Command::Ingest(args) => {
            let paths = ingest_paths(&args.path, args.recursive)?;
            let mut outcomes = Vec::new();
            for path in paths {
                outcomes.push(store.ingest(document_from_path(
                    &path,
                    &args.kind,
                    &args.scope,
                    args.title.clone(),
                )?)?);
            }
            emit(&outcomes, cli.json)
        }
        Command::Embed(args) => {
            let provider = build_provider(&args.provider, cli.dimensions, &home)?;
            let mut report = store.observe_embeddings(provider.as_ref(), args.limit)?;
            if args.drain {
                loop {
                    if report.selected == 0 || report.remaining == 0 {
                        break;
                    }
                    let next = store.observe_embeddings(provider.as_ref(), args.limit)?;
                    report.selected += next.selected;
                    report.embedded += next.embedded;
                    report.remaining = next.remaining;
                }
            }
            emit(&report, cli.json)
        }
        Command::Search(args) => {
            let provider = if args.mode == SearchMode::Lexical {
                None
            } else {
                Some(build_provider(&args.provider, cli.dimensions, &home)?)
            };
            let results = store.search(
                &SearchRequest {
                    query: args.query,
                    mode: args.mode,
                    limit: args.limit,
                    scope: args.scope,
                    source_kind: args.kind,
                },
                provider.as_deref(),
            )?;
            emit(&results, cli.json)
        }
        Command::ImportLegacy(args) => {
            let source_home = expand_tilde(&args.source_home)?;
            let report = moon::legacy::import_legacy(
                &mut store,
                &source_home,
                args.include_raw,
                args.dry_run,
            )?;
            emit(&report, cli.json)
        }
        Command::Health => unreachable!("health is handled without creating or migrating storage"),
        Command::Backup(args) => {
            store.backup_to(&args.destination)?;
            emit(
                &serde_json::json!({"ok": true, "destination": args.destination}),
                cli.json,
            )
        }
        Command::Export(args) => {
            let exported = store.export_memories(&args.destination)?;
            emit(
                &serde_json::json!({
                    "ok": true,
                    "destination": args.destination,
                    "exported": exported,
                }),
                cli.json,
            )
        }
        Command::Shadow(args) => {
            let provider = build_provider(&args.provider, cli.dimensions, &home)?;
            let native = store.search(
                &SearchRequest {
                    query: args.query.clone(),
                    mode: SearchMode::Hybrid,
                    limit: args.limit,
                    scope: args.scope,
                    source_kind: None,
                },
                Some(provider.as_ref()),
            )?;
            let legacy_home = expand_tilde(&args.legacy_home)?;
            let legacy = moon::legacy::search_legacy(&legacy_home, &args.query, args.limit)?;
            let common_source_count = native
                .iter()
                .filter(|hit| {
                    legacy.iter().any(|legacy_hit| {
                        hit.source_uri
                            .strip_prefix("legacy://")
                            .is_some_and(|path| Path::new(path) == legacy_hit.path)
                    })
                })
                .count();
            emit(
                &moon::ShadowReport {
                    query: args.query,
                    native,
                    legacy,
                    common_source_count,
                },
                cli.json,
            )
        }
        Command::RebuildFts => {
            let indexed = store.rebuild_fts()?;
            emit(
                &serde_json::json!({"ok": true, "indexed": indexed}),
                cli.json,
            )
        }
        Command::RequeueEmbeddings => {
            let queued = store.requeue_embeddings()?;
            emit(&serde_json::json!({"ok": true, "queued": queued}), cli.json)
        }
        Command::State(args) => match args.command {
            StateCommand::Get { key } => emit(
                &serde_json::json!({"key": key, "value": store.get_state(&key)?}),
                cli.json,
            ),
            StateCommand::Set { key, value_json } => {
                let value = serde_json::from_str(&value_json)
                    .context("value_json must contain valid JSON")?;
                store.set_state(&key, &value)?;
                emit(&serde_json::json!({"ok": true, "key": key}), cli.json)
            }
        },
        Command::Benchmark(args) => {
            let iterations = args.iterations.clamp(1, 10_000);
            let provider = if args.mode == SearchMode::Lexical {
                None
            } else {
                Some(build_provider(&args.provider, cli.dimensions, &home)?)
            };
            let request = SearchRequest {
                query: args.query,
                mode: args.mode,
                limit: args.limit,
                scope: None,
                source_kind: None,
            };
            let _ = store.search(&request, provider.as_deref())?;
            let mut latencies = Vec::with_capacity(iterations);
            for _ in 0..iterations {
                let start = Instant::now();
                let _ = store.search(&request, provider.as_deref())?;
                latencies.push(start.elapsed().as_secs_f64() * 1_000.0);
            }
            latencies.sort_by(f64::total_cmp);
            emit(
                &serde_json::json!({
                    "iterations": iterations,
                    "p50_ms": percentile(&latencies, 0.50),
                    "p95_ms": percentile(&latencies, 0.95),
                    "p99_ms": percentile(&latencies, 0.99),
                }),
                cli.json,
            )
        }
        Command::Metrics(args) => match args.command {
            MetricsCommand::Summary { since } => {
                let since_ms = since_timestamp_ms(&since)?;
                emit(&store.metrics_summary(since_ms)?, cli.json)
            }
            MetricsCommand::Recent { since, limit } => {
                let since_ms = since_timestamp_ms(&since)?;
                emit(&store.context_metrics_recent(since_ms, limit)?, cli.json)
            }
            MetricsCommand::Review {
                request,
                outcome,
                expected_rank,
            } => emit(
                &store.review_context_metric(&request, outcome, expected_rank)?,
                cli.json,
            ),
            MetricsCommand::MarkInjection { request, injected } => {
                store.mark_context_injected(&request, injected)?;
                emit(
                    &serde_json::json!({"ok": true, "request_id": request}),
                    cli.json,
                )
            }
            MetricsCommand::RecordRuntime {
                kind,
                status,
                duration_us,
                evidence_changed,
                learning_eligible,
                proposed_memories,
                accepted_memories,
                compacted,
                tokens_before,
                tokens_after,
            } => {
                let is_learning = kind == "learning";
                let is_compaction = kind == "compaction";
                let event_id = store.record_runtime_metric(&RuntimeMetricInput {
                    event_kind: kind,
                    status,
                    duration_us,
                    evidence_changed: is_learning.then_some(evidence_changed),
                    learning_eligible: is_learning.then_some(learning_eligible),
                    proposed_memories,
                    accepted_memories,
                    compacted: is_compaction.then_some(compacted),
                    tokens_before,
                    tokens_after,
                    ..RuntimeMetricInput::default()
                })?;
                emit(
                    &serde_json::json!({"ok": true, "event_id": event_id}),
                    cli.json,
                )
            }
            MetricsCommand::Export { since, destination } => {
                let since_ms = since_timestamp_ms(&since)?;
                let exported = store.export_metrics(&destination, since_ms)?;
                emit(
                    &serde_json::json!({
                        "ok": true,
                        "destination": destination,
                        "exported": exported,
                        "redacted": true,
                    }),
                    cli.json,
                )
            }
            MetricsCommand::Prune { older_than, yes } => {
                let before_ms = since_timestamp_ms(&older_than)?;
                let matched = store.prune_metrics(before_ms, yes)?;
                emit(
                    &serde_json::json!({
                        "ok": true,
                        "matched": matched,
                        "deleted": if yes { matched } else { 0 },
                        "changed": yes && matched > 0,
                        "dry_run": !yes,
                        "before_ms": before_ms,
                    }),
                    cli.json,
                )
            }
        },
        Command::Serve(args) => {
            if !args.stdio {
                anyhow::bail!("only the private --stdio transport is supported");
            }
            let provider = build_provider(&args.provider, cli.dimensions, &home)?;
            let stdin = io::stdin();
            let stdout = io::stdout();
            moon::server::serve_stdio(&mut store, provider.as_ref(), stdin.lock(), stdout.lock())
        }
        Command::Update(_) => unreachable!("update is handled without opening storage for writes"),
    }
}

fn run_update(home: &Path, dimensions: usize, json: bool, args: &UpdateArgs) -> Result<()> {
    let client = moon::update::ReleaseClient::production()?;
    let release = client.fetch_release(args.version.as_deref())?;
    let check =
        moon::update::check_for_update(home, dimensions, args.version.as_deref(), &release)?;
    if args.check {
        return emit(&check, json);
    }

    let identity = moon::version::VersionInfo::current_for_home(home)?;
    let context = moon::update::ApplyContext {
        home: home.to_path_buf(),
        dimensions,
        identity,
        openclaw: moon::update::inspect_openclaw_config()?,
        skill_path: moon::update::default_skill_path()?,
        allow_downgrade: args.allow_downgrade,
    };
    let asset = release.asset_for_current_target()?;
    let archive = client.fetch_archive(&release, asset)?;
    let openclaw = moon::update::SystemOpenClaw::discover()?;
    let plan = moon::update::preflight_update(&context, &release, &archive, &openclaw)?;
    if args.dry_run {
        return emit(
            &serde_json::json!({
                "ok": true,
                "changed": false,
                "dry_run": true,
                "check": check,
                "plan": plan,
                "archive_verified": true,
            }),
            json,
        );
    }

    if !args.yes {
        if json || !io::stdin().is_terminal() {
            return moon::update::fail(
                "authorization_required",
                "applying an update non-interactively requires --yes",
            );
        }
        println!("{}", serde_json::to_string_pretty(&plan)?);
        print!("Apply this Moon update? [y/N] ");
        io::stdout().flush()?;
        let mut response = String::new();
        io::stdin().read_line(&mut response)?;
        if !matches!(response.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            return moon::update::fail("update_cancelled", "update cancelled before mutation");
        }
    }

    let result = moon::update::apply_update(&context, &release, &archive, &openclaw)?;
    emit(&result, json)
}

fn resolve_home(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path.to_path_buf());
    }
    let executable_name = env::current_exe().ok().and_then(|path| {
        path.file_stem()
            .and_then(|value| value.to_str())
            .map(str::to_owned)
    });
    let runtime_name = default_runtime_name(executable_name.as_deref());
    dirs::home_dir()
        .map(|home| home.join(runtime_name))
        .ok_or_else(|| anyhow::anyhow!("home directory could not be resolved"))
}

fn default_runtime_name(_executable_name: Option<&str>) -> &'static str {
    ".moon"
}

fn expand_tilde(value: &str) -> Result<PathBuf> {
    if value == "~" {
        return dirs::home_dir().ok_or_else(|| anyhow::anyhow!("home directory unavailable"));
    }
    if let Some(rest) = value.strip_prefix("~/") {
        return dirs::home_dir()
            .map(|home| home.join(rest))
            .ok_or_else(|| anyhow::anyhow!("home directory unavailable"));
    }
    Ok(PathBuf::from(value))
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DistillProposalPayload {
    content: String,
    evidence_quote: String,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum DistillBatchPayload {
    Array(Vec<DistillBatchProposal>),
    Object { memories: Vec<DistillBatchProposal> },
}

#[derive(Debug, Deserialize)]
struct DistillBatchProposal {
    canonical_key: String,
    #[serde(default = "default_memory_kind")]
    kind: String,
    title: Option<String>,
    content: String,
    evidence_quote: String,
    #[serde(default = "default_importance")]
    importance: f64,
    #[serde(default = "default_confidence")]
    confidence: f64,
    #[serde(default)]
    pinned: bool,
    #[serde(default)]
    supersedes_document_id: Option<i64>,
}

fn default_memory_kind() -> String {
    "fact".to_string()
}

fn default_importance() -> f64 {
    0.5
}

fn default_confidence() -> f64 {
    1.0
}

fn read_explicit_content(content: Option<String>, file: Option<&Path>) -> Result<String> {
    match (content, file) {
        (Some(content), None) if !content.trim().is_empty() => {
            if content.len() as u64 > MAX_INPUT_BYTES {
                anyhow::bail!("content exceeds the maximum size of {MAX_INPUT_BYTES} bytes");
            }
            Ok(content)
        }
        (Some(_), None) => anyhow::bail!("content must not be empty"),
        (None, Some(path)) => read_bounded_text(path),
        (None, None) => read_bounded_stdin("content"),
        (Some(_), Some(_)) => unreachable!("clap rejects conflicting content inputs"),
    }
}

fn read_bounded_stdin(label: &str) -> Result<String> {
    let mut input = io::stdin().take(MAX_INPUT_BYTES + 1);
    let mut value = String::new();
    input
        .read_to_string(&mut value)
        .with_context(|| format!("failed to read {label} from stdin"))?;
    if value.len() as u64 > MAX_INPUT_BYTES {
        anyhow::bail!("{label} exceeds the maximum size of {MAX_INPUT_BYTES} bytes");
    }
    if value.trim().is_empty() {
        anyhow::bail!("{label} must not be empty");
    }
    Ok(value)
}

fn read_bounded_text(path: &Path) -> Result<String> {
    let metadata =
        fs::metadata(path).with_context(|| format!("failed to inspect {}", path.display()))?;
    if metadata.len() > MAX_INPUT_BYTES {
        anyhow::bail!(
            "{} exceeds the maximum input size of {MAX_INPUT_BYTES} bytes",
            path.display()
        );
    }
    fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))
}

fn ingest_paths(path: &Path, recursive: bool) -> Result<Vec<PathBuf>> {
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }
    if !path.is_dir() {
        anyhow::bail!("ingest path does not exist: {}", path.display());
    }
    if !recursive {
        anyhow::bail!("directory ingestion requires --recursive");
    }
    let mut paths = WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .filter(|entry| {
            entry
                .path()
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| {
                    ["md", "txt", "jsonl"]
                        .iter()
                        .any(|ext| value.eq_ignore_ascii_case(ext))
                })
        })
        .map(|entry| entry.into_path())
        .collect::<Vec<_>>();
    paths.sort();
    Ok(paths)
}

fn document_from_path(
    path: &Path,
    kind: &str,
    scope: &str,
    explicit_title: Option<String>,
) -> Result<IngestDocument> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", path.display()))?;
    let content = read_bounded_text(&canonical)?;
    let metadata = fs::metadata(&canonical)?;
    let modified_at_ms = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0);
    let title = explicit_title.or_else(|| {
        canonical
            .file_stem()
            .and_then(|value| value.to_str())
            .map(ToString::to_string)
    });
    Ok(IngestDocument {
        source_uri: format!("file://{}", canonical.display()),
        source_kind: kind.to_string(),
        scope: scope.to_string(),
        title,
        content,
        modified_at_ms,
        metadata_json: serde_json::json!({"path": canonical}).to_string(),
    })
}

fn build_provider(
    args: &ProviderArgs,
    dimensions: usize,
    home: &Path,
) -> Result<Box<dyn EmbeddingProvider>> {
    match args.provider.trim().to_ascii_lowercase().as_str() {
        "hash" | "offline" => Ok(Box::new(HashEmbedding::new(dimensions))),
        "local" | "multilingual" => Ok(Box::new(LocalEmbedding::new(
            &home.join("models/fastembed"),
            dimensions,
        )?)),
        unknown => anyhow::bail!("unknown embedding provider `{unknown}`"),
    }
}

fn emit(value: &impl Serialize, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string(value)?);
    } else {
        println!("{}", serde_json::to_string_pretty(value)?);
    }
    Ok(())
}

fn percentile(sorted: &[f64], percentile: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let index = ((sorted.len() - 1) as f64 * percentile).round() as usize;
    sorted[index]
}

fn current_time_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn since_timestamp_ms(value: &str) -> Result<i64> {
    let value = value.trim();
    let (number, multiplier) = if let Some(number) = value.strip_suffix('d') {
        (number, 86_400_000i64)
    } else if let Some(number) = value.strip_suffix('h') {
        (number, 3_600_000i64)
    } else if let Some(number) = value.strip_suffix('m') {
        (number, 60_000i64)
    } else if let Some(number) = value.strip_suffix('s') {
        (number, 1_000i64)
    } else {
        anyhow::bail!("duration must use a d, h, m, or s suffix (for example 7d or 12h)");
    };
    let amount = number
        .parse::<i64>()
        .context("duration must start with a positive whole number")?;
    if amount <= 0 {
        anyhow::bail!("duration must be greater than zero");
    }
    let duration_ms = amount
        .checked_mul(multiplier)
        .context("duration is too large")?;
    Ok(current_time_ms().saturating_sub(duration_ms))
}

#[cfg(test)]
mod tests {
    use super::default_runtime_name;

    #[test]
    fn installed_moon_uses_the_production_runtime_name() {
        assert_eq!(default_runtime_name(Some("moon")), ".moon");
        assert_eq!(default_runtime_name(Some("renamed-moon")), ".moon");
        assert_eq!(default_runtime_name(None), ".moon");
    }
}
