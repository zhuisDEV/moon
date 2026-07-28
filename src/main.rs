use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use moon::redaction::redact_text;
use moon::{
    AuthResolver, ContextRequest, DistillInput, EmbeddingProvider, EvidenceInput, HashEmbedding,
    IngestDocument, LocalEmbedding, MemoryInput, SearchMode, SearchRequest, Store,
};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::io::{self, Read};
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
    /// Inspect or establish the Codex authentication fallback chain.
    Auth(AuthArgs),
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
    /// Keep the local embedding model warm over a private JSON-lines channel.
    Serve(ServeArgs),
}

#[derive(Debug, Args)]
struct AuthArgs {
    #[command(subcommand)]
    command: AuthCommand,
}

#[derive(Debug, Subcommand)]
enum AuthCommand {
    /// Report OpenClaw, Moon, and local Codex authentication availability.
    Status {
        /// Set only when an OpenClaw adapter has verified its model runtime.
        #[arg(long)]
        openclaw_available: bool,
    },
    /// Log in through Codex using Moon's isolated credential store.
    Login {
        /// Use the Codex device-code flow instead of opening a browser.
        #[arg(long)]
        device_auth: bool,
    },
    /// Run one bounded model request through Moon then local Codex auth.
    Exec {
        #[arg(long, conflicts_with = "file")]
        prompt: Option<String>,
        #[arg(long, conflicts_with = "prompt")]
        file: Option<PathBuf>,
        #[arg(long, default_value = "gpt-5.6-sol")]
        model: String,
        #[arg(long, default_value = "high")]
        reasoning: String,
    },
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
struct ServeArgs {
    #[command(flatten)]
    provider: ProviderArgs,
    /// Read and write one JSON object per line over stdin/stdout.
    #[arg(long, default_value_t = true)]
    stdio: bool,
}

fn main() {
    let wants_json = env::args_os().any(|argument| argument == "--json");
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
        if wants_json {
            eprintln!(
                "{}",
                serde_json::json!({
                    "ok": false,
                    "error": {
                        "code": "operation_failed",
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
    if let Command::Auth(args) = &cli.command {
        let resolver = AuthResolver::default();
        return match &args.command {
            AuthCommand::Status { openclaw_available } => {
                emit(&resolver.status(&home, *openclaw_available), cli.json)
            }
            AuthCommand::Login { device_auth } => {
                emit(&resolver.login(&home, *device_auth)?, cli.json)
            }
            AuthCommand::Exec {
                prompt,
                file,
                model,
                reasoning,
            } => {
                let prompt = read_model_prompt(prompt.clone(), file.as_deref())?;
                emit(
                    &resolver.execute(&home, &prompt, model, reasoning)?,
                    cli.json,
                )
            }
        };
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
        Command::Auth(_) => unreachable!("auth is handled without opening storage"),
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
            let packet = store.assemble_context(
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
            if cli.json {
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
            let mut report = store.embed_pending(provider.as_ref(), args.limit)?;
            if args.drain {
                loop {
                    if report.selected == 0 || report.remaining == 0 {
                        break;
                    }
                    let next = store.embed_pending(provider.as_ref(), args.limit)?;
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
        Command::Serve(args) => {
            if !args.stdio {
                anyhow::bail!("only the private --stdio transport is supported");
            }
            let provider = build_provider(&args.provider, cli.dimensions, &home)?;
            let stdin = io::stdin();
            let stdout = io::stdout();
            moon::server::serve_stdio(&mut store, provider.as_ref(), stdin.lock(), stdout.lock())
        }
    }
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

fn read_model_prompt(prompt: Option<String>, file: Option<&Path>) -> Result<String> {
    match (prompt, file) {
        (Some(prompt), None) => Ok(prompt),
        (None, Some(path)) => read_bounded_text(path),
        (None, None) => read_bounded_stdin("model prompt"),
        (Some(_), Some(_)) => unreachable!("clap rejects conflicting prompt inputs"),
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
