use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crate::commands::CommandReport;
use crate::moon::util::{run_command_with_timeout, truncate_with_ellipsis};

const MOON_REPO_URL: &str = "https://github.com/zhuisdev/moon.git";
const PRESERVED_RUNTIME_FILES: [(&str, &str); 2] = [("env", ".env"), ("moon_toml", "moon.toml")];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateChannel {
    Stable,
    Main,
}

impl UpdateChannel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Main => "main",
        }
    }
}

#[derive(Debug, Clone)]
pub struct UpdateOptions {
    pub check: bool,
    pub dry_run: bool,
    pub channel: UpdateChannel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Semver {
    major: u64,
    minor: u64,
    patch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReleaseTag {
    raw_tag: String,
    version: Semver,
}

#[derive(Debug, Clone)]
struct FileSnapshot {
    label: &'static str,
    path: PathBuf,
    original: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
enum UpdateTarget {
    Stable(ReleaseTag),
    Main,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PluginAlignmentStrategy {
    InProcess,
    DeferredToInstalledBinary,
}

impl UpdateTarget {
    fn describe(&self) -> String {
        match self {
            Self::Stable(tag) => format!("stable tag {}", tag.raw_tag),
            Self::Main => "main branch".to_string(),
        }
    }
}

pub fn run(opts: &UpdateOptions) -> Result<CommandReport> {
    let mut report = CommandReport::new("update");
    let moon_paths = crate::moon::paths::resolve_paths()?;
    let moon_home = moon_paths.moon_home.clone();
    let current_version = env!("CARGO_PKG_VERSION").to_string();
    let current_semver = parse_semver(&current_version);

    report.detail(format!("channel={}", opts.channel.as_str()));
    report.detail(format!("moon_home={}", moon_home.display()));
    report.detail(format!("current_version={current_version}"));

    let snapshots = match capture_file_snapshots(&moon_home, &mut report) {
        Ok(snapshots) => snapshots,
        Err(err) => {
            report.issue(format!("failed to snapshot runtime config files: {err:#}"));
            return Ok(report);
        }
    };

    let target = match resolve_target(opts.channel) {
        Ok(target) => target,
        Err(err) => {
            report.issue(format!("failed to resolve update target: {err:#}"));
            return Ok(report);
        }
    };
    report.detail(format!("target={}", target.describe()));

    if opts.check {
        report.detail("check_only=true".to_string());
        return Ok(report);
    }

    if opts.dry_run {
        report.detail("dry_run=true".to_string());
        report.detail(format!("plan: {}", cargo_install_command_preview(&target)));
        report.detail("plan: moon --allow-out-of-bounds install".to_string());
        report.detail("plan: moon --allow-out-of-bounds verify --strict".to_string());
        return Ok(report);
    }

    let should_install = match (&target, current_semver) {
        (UpdateTarget::Stable(latest), Some(current)) if current == latest.version => {
            report.detail(format!(
                "binary already at latest stable version ({})",
                latest.raw_tag
            ));
            false
        }
        _ => true,
    };

    if should_install && !run_cargo_install(&target, &mut report)? {
        if let Err(err) = restore_file_snapshots(&snapshots, &mut report) {
            report.issue(format!(
                "failed to restore preserved runtime files: {err:#}"
            ));
        }
        return Ok(report);
    }

    let moon_bin = resolve_moon_binary_path(&mut report)?;
    let install_ok = run_moon_subcommand(
        &moon_bin,
        &moon_home,
        &["--allow-out-of-bounds", "install"],
        "install",
        &mut report,
    )?;

    if install_ok {
        match plugin_alignment_strategy(should_install) {
            PluginAlignmentStrategy::InProcess => {
                crate::commands::align_openclaw_plugin_state(&mut report, "update");
            }
            PluginAlignmentStrategy::DeferredToInstalledBinary => {
                report.detail(
                    "openclaw.plugin_alignment.status=deferred reason=updated_binary_required"
                        .to_string(),
                );
                report.detail(
                    "openclaw.plugin_alignment.note=post-install plugin validation runs via the newly installed moon binary during verify --strict"
                        .to_string(),
                );
            }
        }
        let _ = run_moon_subcommand(
            &moon_bin,
            &moon_home,
            &["--allow-out-of-bounds", "verify", "--strict"],
            "verify_strict",
            &mut report,
        )?;
    }

    if let Err(err) = restore_file_snapshots(&snapshots, &mut report) {
        report.issue(format!(
            "failed to restore preserved runtime files: {err:#}"
        ));
    }

    Ok(report)
}

fn resolve_target(channel: UpdateChannel) -> Result<UpdateTarget> {
    match channel {
        UpdateChannel::Main => Ok(UpdateTarget::Main),
        UpdateChannel::Stable => Ok(UpdateTarget::Stable(resolve_latest_stable_tag()?)),
    }
}

fn plugin_alignment_strategy(should_install: bool) -> PluginAlignmentStrategy {
    if should_install {
        PluginAlignmentStrategy::DeferredToInstalledBinary
    } else {
        PluginAlignmentStrategy::InProcess
    }
}

fn resolve_latest_stable_tag() -> Result<ReleaseTag> {
    let mut cmd = Command::new("git");
    cmd.args(["ls-remote", "--tags", "--refs", MOON_REPO_URL]);
    let output = run_command_with_timeout(&mut cmd)
        .context("failed to query remote tags via `git ls-remote --tags --refs`")?;
    if !output.status.success() {
        anyhow::bail!(
            "remote tag query failed: {}",
            summarize_command_failure(&output)
        );
    }

    select_latest_release_tag(&String::from_utf8_lossy(&output.stdout))
        .context("no release-like tags (vX.Y.Z) found on remote")
}

fn select_latest_release_tag(raw: &str) -> Option<ReleaseTag> {
    raw.lines()
        .filter_map(parse_release_tag_from_ls_remote_line)
        .max_by(|left, right| left.version.cmp(&right.version))
}

fn parse_release_tag_from_ls_remote_line(line: &str) -> Option<ReleaseTag> {
    let mut cols = line.split_whitespace();
    let _hash = cols.next()?;
    let ref_name = cols.next()?;
    let tag = ref_name.strip_prefix("refs/tags/")?;
    let version = parse_semver(tag)?;
    Some(ReleaseTag {
        raw_tag: tag.to_string(),
        version,
    })
}

fn parse_semver(raw: &str) -> Option<Semver> {
    let trimmed = raw.strip_prefix('v').unwrap_or(raw).trim();
    if trimmed.is_empty() || trimmed.contains('-') || trimmed.contains('+') {
        return None;
    }

    let mut parts = trimmed.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next()?.parse::<u64>().ok()?;
    let patch = parts.next()?.parse::<u64>().ok()?;
    if parts.next().is_some() {
        return None;
    }

    Some(Semver {
        major,
        minor,
        patch,
    })
}

fn run_cargo_install(target: &UpdateTarget, report: &mut CommandReport) -> Result<bool> {
    let mut cmd = Command::new("cargo");
    cmd.arg("install").arg("--git").arg(MOON_REPO_URL);
    match target {
        UpdateTarget::Stable(tag) => {
            cmd.arg("--tag").arg(&tag.raw_tag);
        }
        UpdateTarget::Main => {
            cmd.arg("--branch").arg("main");
        }
    }
    cmd.arg("moon").arg("--force");

    report.detail(format!("exec={}", render_command(&cmd)));
    let output = run_command_with_timeout(&mut cmd)
        .context("failed to run cargo install for moon update")?;
    if !output.status.success() {
        report.issue(format!(
            "cargo install failed: {}",
            summarize_command_failure(&output)
        ));
        return Ok(false);
    }

    report.detail("cargo_install=ok".to_string());
    Ok(true)
}

fn resolve_moon_binary_path(report: &mut CommandReport) -> Result<PathBuf> {
    if let Ok(path) = which::which("moon") {
        report.detail(format!("moon_binary={}", path.display()));
        return Ok(path);
    }

    let fallback = std::env::current_exe().context("failed to resolve current executable path")?;
    report.detail(format!(
        "moon_binary=fallback_current_exe ({})",
        fallback.display()
    ));
    Ok(fallback)
}

fn run_moon_subcommand(
    moon_bin: &Path,
    moon_home: &Path,
    args: &[&str],
    label: &str,
    report: &mut CommandReport,
) -> Result<bool> {
    let mut cmd = Command::new(moon_bin);
    cmd.args(args)
        .current_dir(moon_home)
        .env("MOON_HOME", moon_home.display().to_string());

    report.detail(format!("{label}.exec={}", render_command(&cmd)));
    let output = run_command_with_timeout(&mut cmd)
        .with_context(|| format!("failed to run `moon {}`", args.join(" ")))?;

    if output.status.success() {
        report.detail(format!("{label}=ok"));
        return Ok(true);
    }

    report.issue(format!(
        "{label}=failed {}",
        summarize_command_failure(&output)
    ));
    Ok(false)
}

fn capture_file_snapshots(
    moon_home: &Path,
    report: &mut CommandReport,
) -> Result<Vec<FileSnapshot>> {
    let mut snapshots = Vec::new();
    for (label, file_name) in PRESERVED_RUNTIME_FILES {
        let path = moon_home.join(file_name);
        let original = if path.is_file() {
            Some(
                fs::read(&path)
                    .with_context(|| format!("failed to read snapshot for {}", path.display()))?,
            )
        } else {
            None
        };

        match original {
            Some(_) => report.detail(format!("preserve.{label}=tracked ({})", path.display())),
            None => report.detail(format!(
                "preserve.{label}=absent_before_update ({})",
                path.display()
            )),
        }

        snapshots.push(FileSnapshot {
            label,
            path,
            original,
        });
    }
    Ok(snapshots)
}

fn restore_file_snapshots(snapshots: &[FileSnapshot], report: &mut CommandReport) -> Result<()> {
    for snapshot in snapshots {
        let Some(original) = &snapshot.original else {
            if snapshot.path.is_file() {
                report.detail(format!(
                    "preserve.{}=kept_new_file ({})",
                    snapshot.label,
                    snapshot.path.display()
                ));
            }
            continue;
        };

        let needs_restore = match fs::read(&snapshot.path) {
            Ok(current) => current != *original,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => true,
            Err(err) => {
                return Err(err).with_context(|| {
                    format!(
                        "failed to inspect preserved file {}",
                        snapshot.path.display()
                    )
                });
            }
        };

        if needs_restore {
            if let Some(parent) = snapshot.path.parent() {
                fs::create_dir_all(parent).with_context(|| {
                    format!("failed to create parent directory {}", parent.display())
                })?;
            }
            fs::write(&snapshot.path, original)
                .with_context(|| format!("failed to restore {}", snapshot.path.display()))?;
            report.detail(format!(
                "preserve.{}=restored ({})",
                snapshot.label,
                snapshot.path.display()
            ));
        } else {
            report.detail(format!(
                "preserve.{}=unchanged ({})",
                snapshot.label,
                snapshot.path.display()
            ));
        }
    }

    Ok(())
}

fn cargo_install_command_preview(target: &UpdateTarget) -> String {
    match target {
        UpdateTarget::Stable(tag) => format!(
            "cargo install --git {} --tag {} moon --force",
            MOON_REPO_URL, tag.raw_tag
        ),
        UpdateTarget::Main => format!(
            "cargo install --git {} --branch main moon --force",
            MOON_REPO_URL
        ),
    }
}

fn render_command(cmd: &Command) -> String {
    let mut rendered = cmd.get_program().to_string_lossy().to_string();
    for arg in cmd.get_args() {
        rendered.push(' ');
        rendered.push_str(&arg.to_string_lossy());
    }
    rendered
}

fn summarize_command_failure(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stderr.is_empty() {
        return truncate_with_ellipsis(&stderr, 240);
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !stdout.is_empty() {
        return truncate_with_ellipsis(&stdout, 240);
    }

    match output.status.code() {
        Some(code) => format!("exit code {code}"),
        None => "terminated by signal".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PluginAlignmentStrategy, capture_file_snapshots, parse_semver, plugin_alignment_strategy,
        restore_file_snapshots, select_latest_release_tag,
    };
    use crate::commands::CommandReport;
    use std::fs;

    #[test]
    fn parse_semver_accepts_release_tags() {
        assert!(parse_semver("v1.2.3").is_some());
        assert!(parse_semver("1.2.3").is_some());
    }

    #[test]
    fn parse_semver_rejects_non_release_tags() {
        assert!(parse_semver("v1.2").is_none());
        assert!(parse_semver("v1.2.3-rc1").is_none());
        assert!(parse_semver("feature-x").is_none());
    }

    #[test]
    fn select_latest_release_tag_prefers_highest_semver() {
        let raw = "\
aaaa refs/tags/v1.0.1
bbbb refs/tags/v1.0.10
cccc refs/tags/v1.0.2
dddd refs/tags/not-a-release
";

        let selected = select_latest_release_tag(raw).expect("latest tag");
        assert_eq!(selected.raw_tag, "v1.0.10");
    }

    #[test]
    fn restore_snapshots_puts_original_file_bytes_back() {
        let temp = tempfile::tempdir().expect("tempdir");
        let moon_home = temp.path();
        let env_path = moon_home.join(".env");
        let toml_path = moon_home.join("moon.toml");

        fs::write(&env_path, "ORIGINAL_ENV=1\n").expect("write env");
        fs::write(&toml_path, "mode = \"safe\"\n").expect("write toml");

        let mut report = CommandReport::new("test");
        let snapshots = capture_file_snapshots(moon_home, &mut report).expect("snapshot");

        fs::write(&env_path, "ORIGINAL_ENV=2\n").expect("rewrite env");
        fs::write(&toml_path, "mode = \"changed\"\n").expect("rewrite toml");

        restore_file_snapshots(&snapshots, &mut report).expect("restore");
        assert_eq!(
            fs::read_to_string(&env_path).expect("read env"),
            "ORIGINAL_ENV=1\n"
        );
        assert_eq!(
            fs::read_to_string(&toml_path).expect("read toml"),
            "mode = \"safe\"\n"
        );
    }

    #[test]
    fn restore_snapshots_keeps_new_file_when_absent_before_update() {
        let temp = tempfile::tempdir().expect("tempdir");
        let moon_home = temp.path();
        let toml_path = moon_home.join("moon.toml");

        let mut report = CommandReport::new("test");
        let snapshots = capture_file_snapshots(moon_home, &mut report).expect("snapshot");
        fs::write(&toml_path, "generated = true\n").expect("write new file");

        restore_file_snapshots(&snapshots, &mut report).expect("restore");
        assert_eq!(
            fs::read_to_string(&toml_path).expect("read toml"),
            "generated = true\n"
        );
    }

    #[test]
    fn plugin_alignment_is_deferred_when_update_installs_new_binary() {
        assert_eq!(
            plugin_alignment_strategy(true),
            PluginAlignmentStrategy::DeferredToInstalledBinary
        );
    }

    #[test]
    fn plugin_alignment_runs_in_process_when_no_install_occurs() {
        assert_eq!(
            plugin_alignment_strategy(false),
            PluginAlignmentStrategy::InProcess
        );
    }
}
