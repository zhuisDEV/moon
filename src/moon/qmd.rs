use anyhow::{Context, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::moon::config::MoonHotCollectionLifecycleCommandMode;
use crate::moon::paths::MoonPaths;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbedCapability {
    Bounded,
    UnboundedOnly,
    Missing,
}

impl EmbedCapability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bounded => "bounded",
            Self::UnboundedOnly => "unbounded-only",
            Self::Missing => "missing",
        }
    }
}

#[derive(Debug, Clone)]
pub struct EmbedCapabilityProbe {
    pub capability: EmbedCapability,
    pub note: String,
}

#[derive(Debug, Clone)]
pub struct EmbedExecResult {
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone)]
pub struct RecallExecResult {
    pub mode: &'static str,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone)]
pub struct CollectionLifecycleExecResult {
    pub command: String,
    pub used_fallback: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionLifecycleCapability {
    Primary,
    Fallback,
    Missing,
}

impl CollectionLifecycleCapability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Fallback => "fallback",
            Self::Missing => "missing",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CollectionLifecycleCapabilityProbe {
    pub capability: CollectionLifecycleCapability,
    pub note: String,
}

fn output_contains_all_terms(stdout: &str, stderr: &str, terms: &[&str]) -> bool {
    let combined = format!("{stdout}\n{stderr}").to_ascii_lowercase();
    terms
        .iter()
        .all(|term| combined.contains(&term.to_ascii_lowercase()))
}

fn resolve_qmd_bin(bin: &Path) -> Result<PathBuf> {
    if bin.exists() {
        return Ok(bin.to_path_buf());
    }
    if std::env::var("QMD_BIN")
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
    {
        anyhow::bail!("qmd binary not found at explicit QMD_BIN path {}", bin.display());
    }
    let found = which::which("qmd").context("qmd binary not found in QMD_BIN or PATH")?;
    Ok(found)
}

pub fn install_runtime_env(paths: &MoonPaths) {
    unsafe {
        std::env::set_var("INDEX_PATH", &paths.qmd_db);
        std::env::set_var("QMD_CONFIG_DIR", &paths.qmd_config_dir);
    }
}

fn output_indicates_unknown_subcommand(stdout: &str, stderr: &str) -> bool {
    let combined = format!("{stdout}\n{stderr}").to_ascii_lowercase();
    combined.contains("unknown command")
        || combined.contains("unknown subcommand")
        || combined.contains("unrecognized command")
        || combined.contains("invalid command")
        || combined.contains("invalid choice")
}

fn output_indicates_collection_exists(stdout: &str, stderr: &str) -> bool {
    let combined = format!("{stdout}\n{stderr}").to_ascii_lowercase();
    combined.contains("already exists")
        || combined.contains("collection exists")
        || combined.contains("already created")
}

fn output_indicates_collection_missing(stdout: &str, stderr: &str) -> bool {
    let combined = format!("{stdout}\n{stderr}").to_ascii_lowercase();
    combined.contains("not found")
        || combined.contains("no such collection")
        || combined.contains("does not exist")
        || combined.contains("missing collection")
}

fn output_indicates_collection_already_selected(stdout: &str, stderr: &str) -> bool {
    let combined = format!("{stdout}\n{stderr}").to_ascii_lowercase();
    combined.contains("already active")
        || combined.contains("already selected")
        || combined.contains("already using")
}

fn lifecycle_nonzero_is_ok(action: &'static str, stdout: &str, stderr: &str) -> bool {
    match action {
        "create" => output_indicates_collection_exists(stdout, stderr),
        "drop" => output_indicates_collection_missing(stdout, stderr),
        "switch" => output_indicates_collection_already_selected(stdout, stderr),
        _ => false,
    }
}

fn lifecycle_attempts(
    action: &'static str,
    collection_name: &str,
    collection_path: Option<&Path>,
    command_mode: MoonHotCollectionLifecycleCommandMode,
) -> Vec<(Vec<String>, bool)> {
    match command_mode {
        MoonHotCollectionLifecycleCommandMode::Primary => match action {
            "create" => collection_path
                .map(|path| {
                    vec![(
                        vec![
                            "collection".to_string(),
                            "add".to_string(),
                            path.display().to_string(),
                            "--name".to_string(),
                            collection_name.to_string(),
                            "--mask".to_string(),
                            "**/*.md".to_string(),
                        ],
                        false,
                    )]
                })
                .unwrap_or_default(),
            "drop" => vec![(
                vec![
                    "collection".to_string(),
                    "remove".to_string(),
                    collection_name.to_string(),
                ],
                false,
            )],
            _ => Vec::new(),
        },
        MoonHotCollectionLifecycleCommandMode::Fallback => match action {
            "create" => vec![
                (
                    vec!["create".to_string(), collection_name.to_string()],
                    true,
                ),
                (
                    vec![
                        "collections".to_string(),
                        "create".to_string(),
                        collection_name.to_string(),
                    ],
                    true,
                ),
            ],
            "switch" => vec![
                (
                    vec!["switch".to_string(), collection_name.to_string()],
                    true,
                ),
                (vec!["use".to_string(), collection_name.to_string()], true),
                (
                    vec![
                        "collection".to_string(),
                        "use".to_string(),
                        collection_name.to_string(),
                    ],
                    true,
                ),
            ],
            "drop" => vec![
                (vec!["drop".to_string(), collection_name.to_string()], true),
                (
                    vec!["remove".to_string(), collection_name.to_string()],
                    true,
                ),
                (
                    vec![
                        "collections".to_string(),
                        "drop".to_string(),
                        collection_name.to_string(),
                    ],
                    true,
                ),
            ],
            _ => Vec::new(),
        },
    }
}

fn run_collection_lifecycle(
    qmd_bin: &Path,
    action: &'static str,
    collection_name: &str,
    collection_path: Option<&Path>,
    command_mode: MoonHotCollectionLifecycleCommandMode,
    timeout_secs: Option<u64>,
) -> Result<CollectionLifecycleExecResult> {
    let bin = resolve_qmd_bin(qmd_bin)?;
    let attempts_to_run = lifecycle_attempts(action, collection_name, collection_path, command_mode);
    if attempts_to_run.is_empty() {
        anyhow::bail!(
            "invalid qmd lifecycle action `{}` for collection `{}`",
            action,
            collection_name
        );
    }
    let mut attempts = Vec::new();

    for (args, used_fallback) in attempts_to_run {
        let mut cmd = Command::new(&bin);
        cmd.args(&args);
        let output = crate::moon::util::run_command_with_optional_timeout(&mut cmd, timeout_secs)
            .with_context(|| format!("failed to run `{}`", bin.display()))?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let command = args.join(" ");

        if output.status.success() || lifecycle_nonzero_is_ok(action, &stdout, &stderr) {
            return Ok(CollectionLifecycleExecResult {
                command,
                used_fallback,
            });
        }

        let code = output
            .status
            .code()
            .map(|v| v.to_string())
            .unwrap_or_else(|| "signal".to_string());
        attempts.push(format!(
            "cmd=`{}` code={} stderr={}",
            command,
            code,
            crate::moon::util::truncate_with_ellipsis(stderr.trim(), 180)
        ));
        if output_indicates_unknown_subcommand(&stdout, &stderr) {
            continue;
        }

        anyhow::bail!(
            "qmd collection {} failed for `{}`: mode={} cmd=`{}` stderr={}",
            action,
            collection_name,
            command_mode.as_str(),
            command,
            crate::moon::util::truncate_with_ellipsis(stderr.trim(), 220)
        );
    }

    anyhow::bail!(
        "qmd collection {} unsupported for `{}` in {} mode; attempts: {}",
        action,
        collection_name,
        command_mode.as_str(),
        attempts.join(" | ")
    );
}

pub fn probe_collection_lifecycle_capability(
    qmd_bin: &Path,
    command_mode: MoonHotCollectionLifecycleCommandMode,
) -> CollectionLifecycleCapabilityProbe {
    let bin = match resolve_qmd_bin(qmd_bin) {
        Ok(bin) => bin,
        Err(err) => {
            return CollectionLifecycleCapabilityProbe {
                capability: CollectionLifecycleCapability::Missing,
                note: format!("qmd-binary-missing error={err:#}"),
            };
        }
    };

    let run_help = |args: &[&str]| -> Result<(bool, String, String, Option<i32>)> {
        let mut cmd = Command::new(&bin);
        cmd.args(args);
        let output = crate::moon::util::run_command_with_optional_timeout(&mut cmd, Some(20))
            .with_context(|| format!("failed to run `{}`", bin.display()))?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Ok((
            output.status.success(),
            stdout,
            stderr,
            output.status.code(),
        ))
    };

    if command_mode == MoonHotCollectionLifecycleCommandMode::Primary {
        return match run_help(&["collection", "--help"]) {
            Ok((success, stdout, stderr, _code))
                if success && output_contains_all_terms(&stdout, &stderr, &["add", "remove", "show"]) =>
            {
                CollectionLifecycleCapabilityProbe {
                    capability: CollectionLifecycleCapability::Primary,
                    note: "collection-help-supported verbs=add,remove,show".to_string(),
                }
            }
            Ok((false, stdout, stderr, code))
                if !output_indicates_unknown_subcommand(&stdout, &stderr) =>
            {
                CollectionLifecycleCapabilityProbe {
                    capability: CollectionLifecycleCapability::Missing,
                    note: format!(
                        "collection-help-nonzero code={:?} stderr={}",
                        code,
                        crate::moon::util::truncate_with_ellipsis(stderr.trim(), 140)
                    ),
                }
            }
            Ok((true, stdout, stderr, _)) => {
                let mut missing = Vec::new();
                for verb in ["add", "remove", "show"] {
                    if !output_contains_all_terms(&stdout, &stderr, &[verb]) {
                        missing.push(verb);
                    }
                }
                CollectionLifecycleCapabilityProbe {
                    capability: CollectionLifecycleCapability::Missing,
                    note: format!("collection-help-missing-verbs missing={}", missing.join(",")),
                }
            }
            Ok(_) => CollectionLifecycleCapabilityProbe {
                capability: CollectionLifecycleCapability::Missing,
                note: "collection-help-unsupported".to_string(),
            },
            Err(err) => CollectionLifecycleCapabilityProbe {
                capability: CollectionLifecycleCapability::Missing,
                note: format!("collection-help-exec-failed error={err:#}"),
            },
        };
    }

    let fallback_groups = [
        (
            "create",
            vec![
                vec!["create", "--help"],
                vec!["collections", "create", "--help"],
            ],
        ),
        (
            "switch",
            vec![
                vec!["switch", "--help"],
                vec!["use", "--help"],
                vec!["collection", "use", "--help"],
            ],
        ),
        (
            "drop",
            vec![
                vec!["drop", "--help"],
                vec!["remove", "--help"],
                vec!["collections", "drop", "--help"],
            ],
        ),
    ];
    let mut selected = Vec::new();

    for (action, candidates) in fallback_groups {
        let mut found = None::<String>;
        for args in candidates {
            match run_help(&args) {
                Ok((true, _, _, _)) => {
                    found = Some(args.join(" "));
                    break;
                }
                Ok((false, stdout, stderr, _))
                    if !output_indicates_unknown_subcommand(&stdout, &stderr) =>
                {
                    found = Some(args.join(" "));
                    break;
                }
                Ok(_) => {}
                Err(_) => {}
            }
        }
        match found {
            Some(cmd) => selected.push(format!("{action}:{cmd}")),
            None => {
                return CollectionLifecycleCapabilityProbe {
                    capability: CollectionLifecycleCapability::Missing,
                    note: format!("fallback-help-unsupported action={action}"),
                };
            }
        }
    }

    CollectionLifecycleCapabilityProbe {
        capability: CollectionLifecycleCapability::Fallback,
        note: format!("fallback-help-supported {}", selected.join(",")),
    }
}

pub fn collection_create(
    qmd_bin: &Path,
    collection_name: &str,
    collection_path: &Path,
    command_mode: MoonHotCollectionLifecycleCommandMode,
    timeout_secs: Option<u64>,
) -> Result<CollectionLifecycleExecResult> {
    run_collection_lifecycle(
        qmd_bin,
        "create",
        collection_name,
        Some(collection_path),
        command_mode,
        timeout_secs,
    )
}

pub fn collection_drop(
    qmd_bin: &Path,
    collection_name: &str,
    command_mode: MoonHotCollectionLifecycleCommandMode,
    timeout_secs: Option<u64>,
) -> Result<CollectionLifecycleExecResult> {
    run_collection_lifecycle(qmd_bin, "drop", collection_name, None, command_mode, timeout_secs)
}

pub fn probe_embed_capability(qmd_bin: &Path) -> EmbedCapabilityProbe {
    let bin = match resolve_qmd_bin(qmd_bin) {
        Ok(bin) => bin,
        Err(err) => {
            return EmbedCapabilityProbe {
                capability: EmbedCapability::Missing,
                note: format!("qmd-binary-missing error={err:#}"),
            };
        }
    };

    let mut cmd = Command::new(&bin);
    cmd.arg("embed").arg("--help");
    let output = match crate::moon::util::run_command_with_optional_timeout(&mut cmd, Some(30)) {
        Ok(output) => output,
        Err(err) => {
            return EmbedCapabilityProbe {
                capability: EmbedCapability::Missing,
                note: format!("embed-help-exec-failed error={err:#}"),
            };
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let combined = format!("{stdout}\n{stderr}");
    let lower = combined.to_ascii_lowercase();

    if !output.status.success() {
        return EmbedCapabilityProbe {
            capability: EmbedCapability::Missing,
            note: format!(
                "embed-help-nonzero code={:?} stderr={}",
                output.status.code(),
                stderr.trim()
            ),
        };
    }

    if lower.contains("--max-docs") {
        return EmbedCapabilityProbe {
            capability: EmbedCapability::Bounded,
            note: "embed-help-detected-max-docs".to_string(),
        };
    }

    EmbedCapabilityProbe {
        capability: EmbedCapability::UnboundedOnly,
        note: "embed-help-no-max-docs".to_string(),
    }
}

pub fn embed_bounded(
    qmd_bin: &Path,
    collection_name: &str,
    max_docs_per_batch: usize,
    timeout_secs: Option<u64>,
) -> Result<EmbedExecResult> {
    let bin = resolve_qmd_bin(qmd_bin)?;
    let mut cmd = Command::new(&bin);
    cmd.arg("embed")
        .arg(collection_name)
        .arg("--max-docs")
        .arg(max_docs_per_batch.to_string());
    let output = crate::moon::util::run_command_with_optional_timeout(&mut cmd, timeout_secs)
        .with_context(|| format!("failed to run `{}`", bin.display()))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if output.status.success() {
        return Ok(EmbedExecResult { stdout, stderr });
    }

    anyhow::bail!(
        "qmd embed (bounded) failed\nstdout: {}\nstderr: {}",
        stdout,
        stderr
    );
}

pub fn embed_global_batched(
    qmd_bin: &Path,
    max_docs_per_batch: usize,
    timeout_secs: Option<u64>,
) -> Result<EmbedExecResult> {
    let bin = resolve_qmd_bin(qmd_bin)?;
    let mut cmd = Command::new(&bin);
    cmd.arg("embed")
        .arg("--max-docs-per-batch")
        .arg(max_docs_per_batch.to_string());
    let output = crate::moon::util::run_command_with_optional_timeout(&mut cmd, timeout_secs)
        .with_context(|| format!("failed to run `{}`", bin.display()))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if output.status.success() {
        return Ok(EmbedExecResult { stdout, stderr });
    }

    anyhow::bail!(
        "qmd embed (bounded) failed\nstdout: {}\nstderr: {}",
        stdout,
        stderr
    );
}

pub fn output_indicates_embed_status_failed(stdout: &str, stderr: &str) -> bool {
    let combined = format!("{stdout}\n{stderr}");
    let lower = combined.to_ascii_lowercase();

    if lower.contains("\"status\":\"failed\"")
        || lower.contains("\"status\": \"failed\"")
        || lower.contains("\"ok\":false")
        || lower.contains("\"ok\": false")
    {
        return true;
    }

    let Ok(value) = serde_json::from_str::<Value>(stdout) else {
        return false;
    };

    if value
        .get("status")
        .and_then(Value::as_str)
        .is_some_and(|v| v.eq_ignore_ascii_case("failed"))
    {
        return true;
    }
    value
        .get("ok")
        .and_then(Value::as_bool)
        .is_some_and(|ok| !ok)
}

pub fn recall_query(
    qmd_bin: &Path,
    collection_name: &str,
    query: &str,
    limit: usize,
    timeout_secs: Option<u64>,
) -> Result<RecallExecResult> {
    let bin = resolve_qmd_bin(qmd_bin)?;
    for mode in ["query", "search"] {
        let mut cmd = Command::new(&bin);
        cmd.arg(mode)
            .arg(query)
            .arg("--json")
            .arg("-c")
            .arg(collection_name)
            .arg("-n")
            .arg(limit.to_string());

        let output = crate::moon::util::run_command_with_optional_timeout(&mut cmd, timeout_secs)
            .with_context(|| format!("failed to run `{}`", bin.display()))?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if output.status.success() {
            return Ok(RecallExecResult {
                mode,
                stdout,
                stderr,
            });
        }
    }

    anyhow::bail!(
        "qmd recall failed for collection `{}` and query `{}`",
        collection_name,
        query
    );
}
