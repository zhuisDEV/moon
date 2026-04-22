use anyhow::Result;
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::ffi::OsString;

use crate::commands;
use crate::moon::project::ProjectLane;

#[derive(Debug, Parser)]
#[command(name = "moon")]
#[command(about = "MOON v1 context-control and memory CLI")]
#[command(version)]
pub struct Cli {
    #[arg(long, global = true)]
    pub json: bool,

    #[arg(long, global = true)]
    pub allow_out_of_bounds: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Install(InstallArgs),
    Uninstall(UninstallArgs),
    Login(LoginArgs),
    Update(UpdateArgs),
    Verify(VerifyArgs),
    Repair(RepairArgs),
    Status,
    Record(RecordArgs),
    Project(ProjectArgs),
    Cleanse(CleanseArgs),
    Assemble(AssembleArgs),
    #[command(name = "context-engine")]
    ContextEngine(ContextEngineArgs),
    Stop,
    Restart,
    Watch(MoonWatchArgs),
    Recall(RecallArgs),
    Embed(MoonEmbedArgs),
    #[command(name = "distill")]
    Distill(DistillArgs),
    Config(ConfigArgs),
    Health,
}

#[derive(Debug, Args)]
pub struct InstallArgs {
    #[arg(long)]
    pub force: bool,
    #[arg(long)]
    pub dry_run: bool,
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub apply: bool,
}

#[derive(Debug, Args, Default)]
pub struct UninstallArgs {
    #[arg(long)]
    pub dry_run: bool,
    #[arg(long)]
    pub purge: bool,
    #[arg(long)]
    pub remove_binary: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum UpdateChannelArg {
    Stable,
    Main,
}

#[derive(Debug, Args)]
pub struct UpdateArgs {
    #[arg(long)]
    pub check: bool,
    #[arg(long)]
    pub dry_run: bool,
    #[arg(long, value_enum, default_value_t = UpdateChannelArg::Stable)]
    pub channel: UpdateChannelArg,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum LoginProviderArg {
    #[value(name = "openai-codex")]
    OpenAiCodex,
}

#[derive(Debug, Args)]
pub struct LoginArgs {
    #[arg(long, value_enum, default_value_t = LoginProviderArg::OpenAiCodex)]
    pub provider: LoginProviderArg,
    #[arg(long)]
    pub headless: bool,
}

#[derive(Debug, Args, Default)]
pub struct VerifyArgs {
    #[arg(long)]
    pub strict: bool,
    #[arg(long)]
    pub verbose: bool,
}

#[derive(Debug, Args, Default)]
pub struct RepairArgs {
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args, Default)]
pub struct MoonWatchArgs {
    #[arg(long)]
    pub once: bool,
    #[arg(long)]
    pub daemon: bool,
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Args, Default)]
pub struct RecordArgs {
    #[arg(long)]
    pub source: Option<String>,
    #[arg(long = "session-id")]
    pub session_id: Option<String>,
    #[arg(long = "dry-run")]
    pub dry_run: bool,
}

#[derive(Debug, Args, Default)]
pub struct ProjectArgs {
    #[arg(long)]
    pub source: Option<String>,
    #[arg(long = "session-id")]
    pub session_id: Option<String>,
    #[arg(long, default_value = "hot", value_parser = ["hot", "library", "lib"])]
    pub lane: String,
    #[arg(long = "dry-run")]
    pub dry_run: bool,
}

#[derive(Debug, Args, Default)]
pub struct CleanseArgs {
    #[arg(long)]
    pub source: Option<String>,
    #[arg(long = "session-id")]
    pub session_id: Option<String>,
    #[arg(long = "dry-run")]
    pub dry_run: bool,
}

#[derive(Debug, Args, Default)]
pub struct AssembleArgs {
    #[arg(long)]
    pub source: Option<String>,
    #[arg(long = "session-id")]
    pub session_id: Option<String>,
    #[arg(long = "dry-run")]
    pub dry_run: bool,
    #[arg(long = "replay-has-compaction-summary")]
    pub replay_has_compaction_summary: bool,
}

#[derive(Debug, Args, Default)]
pub struct ContextEngineArgs {
    #[arg(long)]
    pub source: Option<String>,
    #[arg(long = "session-id")]
    pub session_id: Option<String>,
    #[arg(long = "used-tokens")]
    pub used_tokens: Option<u64>,
    #[arg(long = "max-tokens")]
    pub max_tokens: Option<u64>,
    #[arg(long = "force-cleanse")]
    pub force_cleanse: bool,
    #[arg(long = "replay-has-compaction-summary")]
    pub replay_has_compaction_summary: bool,
}

#[derive(Debug, Args)]
pub struct RecallArgs {
    #[arg(long)]
    pub query: String,
    #[arg(long, default_value = "history_lib")]
    pub name: String,
    #[arg(long, default_value_t = 5)]
    pub limit: usize,
}

#[derive(Debug, Args)]
pub struct MoonEmbedArgs {
    #[arg(long, default_value = "history_lib")]
    pub name: String,
    #[arg(long, default_value_t = 25)]
    pub max_docs: usize,
    #[arg(long)]
    pub dry_run: bool,
    #[arg(long)]
    pub watcher_trigger: bool,
}

#[derive(Debug, Args)]
pub struct DistillArgs {
    #[arg(long = "mode", default_value = "norm")]
    pub mode: String,
    #[arg(long = "archive")]
    pub archive: Option<String>,
    #[arg(long = "file")]
    pub files: Vec<String>,
    #[arg(long = "session-id")]
    pub session_id: Option<String>,
    #[arg(long = "dry-run")]
    pub dry_run: bool,
}

#[derive(Debug, Args, Default)]
pub struct ConfigArgs {
    #[arg(long)]
    pub show: bool,
}

fn print_report(report: &commands::CommandReport, as_json: bool) -> Result<()> {
    if as_json {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(());
    }

    println!("command: {}", report.command);
    println!("ok: {}", report.ok);
    let verify_failures_first = report.command == "verify" && !report.ok;
    if verify_failures_first {
        if !report.issues.is_empty() {
            println!("issues:");
            for issue in &report.issues {
                println!("- {issue}");
            }
        }
        if !report.details.is_empty() {
            println!("details:");
            for detail in &report.details {
                println!("- {detail}");
            }
        }
    } else {
        if !report.details.is_empty() {
            println!("details:");
            for detail in &report.details {
                println!("- {detail}");
            }
        }
        if !report.issues.is_empty() {
            println!("issues:");
            for issue in &report.issues {
                println!("- {issue}");
            }
        }
    }
    Ok(())
}

fn normalize_single_dash_long_flags() -> Vec<OsString> {
    std::env::args_os()
        .map(|arg| {
            let Some(raw) = arg.to_str() else {
                return arg;
            };

            let rewritten = match raw {
                "-mode" => Some("--mode".to_string()),
                "-archive" => Some("--archive".to_string()),
                "-file" => Some("--file".to_string()),
                "-session-id" => Some("--session-id".to_string()),
                "-lane" => Some("--lane".to_string()),
                "-dry-run" => Some("--dry-run".to_string()),
                "-source" => Some("--source".to_string()),
                "-query" => Some("--query".to_string()),
                "-name" => Some("--name".to_string()),
                "-limit" => Some("--limit".to_string()),
                _ if raw.starts_with("-mode=")
                    || raw.starts_with("-archive=")
                    || raw.starts_with("-file=")
                    || raw.starts_with("-session-id=")
                    || raw.starts_with("-lane=")
                    || raw.starts_with("-dry-run=")
                    || raw.starts_with("-source=")
                    || raw.starts_with("-query=")
                    || raw.starts_with("-name=")
                    || raw.starts_with("-limit=") =>
                {
                    Some(format!("--{}", &raw[1..]))
                }
                _ => None,
            };

            rewritten.map(OsString::from).unwrap_or(arg)
        })
        .collect()
}

fn env_truthy(var: &str) -> bool {
    match std::env::var(var) {
        Ok(value) => {
            let normalized = value.trim().to_ascii_lowercase();
            matches!(normalized.as_str(), "1" | "true" | "yes" | "on")
        }
        Err(_) => false,
    }
}

pub fn run() -> Result<()> {
    let cli = Cli::parse_from(normalize_single_dash_long_flags());
    let paths = crate::moon::paths::resolve_paths()?;
    let allow_out_of_bounds = cli.allow_out_of_bounds || env_truthy("MOON_ALLOW_OUT_OF_BOUNDS");

    // Every command validates CWD except diagnostics.
    match &cli.command {
        Command::Status
        | Command::Health
        | Command::Verify(_)
        | Command::Config(_)
        | Command::Login(_)
        | Command::Uninstall(_)
        | Command::Update(_) => {
            // Diagnostics are exempt from CWD enforcement.
        }
        _ => {
            commands::validate_cwd(&paths, allow_out_of_bounds)?;
        }
    }

    let report = match &cli.command {
        Command::Install(args) => commands::install::run(&commands::install::InstallOptions {
            force: args.force,
            dry_run: args.dry_run,
            apply: args.apply,
        })?,
        Command::Uninstall(args) => {
            commands::moon_uninstall::run(&commands::moon_uninstall::MoonUninstallOptions {
                dry_run: args.dry_run,
                purge: args.purge,
                remove_binary: args.remove_binary,
            })?
        }
        Command::Login(args) => {
            commands::moon_login::run(&commands::moon_login::MoonLoginOptions {
                provider: match args.provider {
                    LoginProviderArg::OpenAiCodex => {
                        commands::moon_login::MoonLoginProvider::OpenAiCodex
                    }
                },
                headless: args.headless,
            })?
        }
        Command::Update(args) => commands::update::run(&commands::update::UpdateOptions {
            check: args.check,
            dry_run: args.dry_run,
            channel: match args.channel {
                UpdateChannelArg::Stable => commands::update::UpdateChannel::Stable,
                UpdateChannelArg::Main => commands::update::UpdateChannel::Main,
            },
        })?,
        Command::Verify(args) => commands::verify::run(&commands::verify::VerifyOptions {
            strict: args.strict,
            verbose: args.verbose,
        })?,
        Command::Repair(args) => {
            commands::repair::run(&commands::repair::RepairOptions { force: args.force })?
        }
        Command::Status => commands::moon_status::run()?,
        Command::Record(args) => {
            commands::moon_record::run(&commands::moon_record::MoonRecordOptions {
                source_path: args.source.clone(),
                session_id: args.session_id.clone(),
                dry_run: args.dry_run,
            })?
        }
        Command::Project(args) => {
            commands::moon_project::run(&commands::moon_project::MoonProjectOptions {
                source_path: args.source.clone(),
                session_id: args.session_id.clone(),
                lane: match args.lane.as_str() {
                    "library" | "lib" => ProjectLane::Library,
                    _ => ProjectLane::Hot,
                },
                dry_run: args.dry_run,
            })?
        }
        Command::Cleanse(args) => {
            commands::moon_cleanse::run(&commands::moon_cleanse::MoonCleanseOptions {
                source_path: args.source.clone(),
                session_id: args.session_id.clone(),
                dry_run: args.dry_run,
            })?
        }
        Command::Assemble(args) => {
            commands::moon_assemble::run(&commands::moon_assemble::MoonAssembleOptions {
                source_path: args.source.clone(),
                session_id: args.session_id.clone(),
                dry_run: args.dry_run,
                replay_has_compaction_summary: args.replay_has_compaction_summary,
            })?
        }
        Command::ContextEngine(args) => commands::moon_context_engine::run(
            &commands::moon_context_engine::MoonContextEngineOptions {
                source_path: args.source.clone(),
                session_id: args.session_id.clone(),
                used_tokens: args.used_tokens,
                max_tokens: args.max_tokens,
                force_cleanse: args.force_cleanse,
                replay_has_compaction_summary: args.replay_has_compaction_summary,
            },
        )?,
        Command::Stop => commands::moon_stop::run()?,
        Command::Restart => commands::moon_restart::run()?,
        Command::Watch(args) => {
            commands::moon_watch::run(&commands::moon_watch::MoonWatchOptions {
                once: args.once,
                daemon: args.daemon,
                dry_run: args.dry_run,
            })?
        }
        Command::Recall(args) => {
            commands::moon_recall::run(&commands::moon_recall::MoonRecallOptions {
                collection_name: args.name.clone(),
                query: args.query.clone(),
                limit: args.limit,
            })?
        }
        Command::Embed(args) => {
            commands::moon_embed::run(&commands::moon_embed::MoonEmbedOptions {
                collection_name: args.name.clone(),
                max_docs: args.max_docs,
                dry_run: args.dry_run,
                watcher_trigger: args.watcher_trigger,
            })?
        }
        Command::Distill(args) => {
            commands::moon_distill::run(&commands::moon_distill::MoonDistillOptions {
                mode: args.mode.clone(),
                archive_path: args.archive.clone(),
                files: args.files.clone(),
                session_id: args.session_id.clone(),
                dry_run: args.dry_run,
            })?
        }
        Command::Config(args) => {
            commands::moon_config::run(&commands::moon_config::MoonConfigOptions {
                show: args.show,
            })?
        }
        Command::Health => commands::moon_health::run()?,
    };

    print_report(&report, cli.json)?;

    if report.ok {
        Ok(())
    } else {
        std::process::exit(2);
    }
}
