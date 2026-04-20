use anyhow::{Context, Result};
use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

use crate::commands::CommandReport;
use crate::commands::moon_stop;
use crate::moon::paths::MoonPaths;
use crate::openclaw::config::{
    read_config_value, remove_moon_installation_config, write_config_atomic,
};

#[derive(Debug, Clone, Default)]
pub struct MoonUninstallOptions {
    pub dry_run: bool,
    pub purge: bool,
    pub remove_binary: bool,
}

pub fn run(opts: &MoonUninstallOptions) -> Result<CommandReport> {
    let moon_paths = crate::moon::paths::resolve_paths()?;
    let openclaw_paths = crate::openclaw::paths::resolve_paths()?;
    let current_exe = env::current_exe().context("failed to resolve current executable path")?;
    let mut report = CommandReport::new("uninstall");

    report.detail(format!("runtime.root={}", moon_paths.moon_home.display()));
    report.detail(format!("uninstall.purge={}", opts.purge));
    report.detail(format!("uninstall.remove_binary={}", opts.remove_binary));

    if opts.dry_run {
        report.detail("daemon.stop=planned".to_string());
    } else {
        report.merge(moon_stop::run()?);
    }

    remove_launchd_artifacts(opts, &mut report)?;
    remove_openclaw_integration(&openclaw_paths, opts, &mut report)?;
    remove_runtime_skills(&openclaw_paths, opts, &mut report)?;
    remove_runtime_artifacts(&moon_paths, opts, &mut report)?;
    remove_shell_profile_block(opts, &mut report)?;
    maybe_remove_binary(&current_exe, opts, &mut report)?;

    Ok(report)
}

fn remove_openclaw_integration(
    paths: &crate::openclaw::paths::OpenClawPaths,
    opts: &MoonUninstallOptions,
    report: &mut CommandReport,
) -> Result<()> {
    if crate::openclaw::gateway::openclaw_available() {
        if opts.dry_run {
            report.detail("openclaw.plugins.uninstall=planned id=moon".to_string());
        } else {
            match crate::openclaw::gateway::try_plugins_uninstall(&paths.plugin_id) {
                Ok(()) => report.detail("openclaw.plugins.uninstall=ok id=moon".to_string()),
                Err(err) => report.issue(format!("openclaw.plugins.uninstall=failed ({err:#})")),
            }
        }
    } else {
        report.detail("openclaw.plugins.uninstall=skipped reason=openclaw_unavailable".to_string());
    }

    if paths.config_path.exists() {
        let mut cfg = read_config_value(paths)?;
        let patch = remove_moon_installation_config(&mut cfg, &paths.plugin_id);
        for key in patch.removed_paths {
            report.detail(format!("removed {key}"));
        }
        if patch.changed {
            if opts.dry_run {
                report.detail(format!(
                    "openclaw.config.update=planned path={}",
                    paths.config_path.display()
                ));
            } else {
                let written = write_config_atomic(paths, &cfg)?;
                report.detail(format!("openclaw.config.updated={written}"));
            }
        } else {
            report.detail("openclaw.config=already_clean".to_string());
        }
    } else {
        report.detail(format!(
            "openclaw.config=skipped reason=missing path={}",
            paths.config_path.display()
        ));
    }

    remove_path_entry(&paths.plugin_dir, opts, report, "openclaw.plugin_dir")?;
    Ok(())
}

fn remove_runtime_skills(
    paths: &crate::openclaw::paths::OpenClawPaths,
    opts: &MoonUninstallOptions,
    report: &mut CommandReport,
) -> Result<()> {
    for path in [
        paths.state_dir.join("skills").join("moon-admin"),
        paths.state_dir.join("skills").join("moon-subagent"),
    ] {
        remove_path_entry(&path, opts, report, "runtime.skill_dir")?;
    }
    Ok(())
}

fn remove_runtime_artifacts(
    paths: &MoonPaths,
    opts: &MoonUninstallOptions,
    report: &mut CommandReport,
) -> Result<()> {
    if opts.purge {
        remove_path_entry(&paths.moon_home, opts, report, "runtime.root.remove")?;
        return Ok(());
    }

    let state_dir = crate::moon::state::state_file_path(paths)
        .parent()
        .map(Path::to_path_buf);
    let qmd_root = paths.qmd_db.parent().map(Path::to_path_buf);
    let removals = [
        paths.raw_dir.clone(),
        paths.mds_dir.clone(),
        paths.mlib_dir.clone(),
        paths.cleanse_dir.clone(),
        paths.logs_dir.clone(),
        paths.context_engine_dir.clone(),
        paths.context_packet_dir.clone(),
        paths.moon_home.join("docs"),
        paths.moon_home.join("README.md"),
        paths.moon_home.join(".env.example"),
        paths.moon_home.join("moon.toml.example"),
    ];

    for path in removals {
        remove_path_entry(&path, opts, report, "runtime.remove")?;
    }
    if let Some(path) = state_dir {
        remove_path_entry(&path, opts, report, "runtime.remove")?;
    }
    if let Some(path) = qmd_root {
        remove_path_entry(&path, opts, report, "runtime.remove")?;
    }

    report.detail(format!(
        "runtime.preserved={}",
        [
            paths.memory_dir.display().to_string(),
            paths.memory_file.display().to_string(),
            paths.moon_home.join(".env").display().to_string(),
            paths.moon_home.join("moon.toml").display().to_string(),
            paths.moon_home.join("auth").display().to_string(),
        ]
        .join(", ")
    ));
    Ok(())
}

fn remove_shell_profile_block(
    opts: &MoonUninstallOptions,
    report: &mut CommandReport,
) -> Result<()> {
    let Some(home_dir) = dirs::home_dir() else {
        report.detail("shell.zprofile=skipped reason=home_dir_unavailable".to_string());
        return Ok(());
    };
    let zprofile_path = home_dir.join(".zprofile");
    if !zprofile_path.exists() {
        report.detail(format!(
            "shell.zprofile=skipped reason=missing path={}",
            zprofile_path.display()
        ));
        return Ok(());
    }

    let existing = fs::read_to_string(&zprofile_path)
        .with_context(|| format!("failed to read {}", zprofile_path.display()))?;
    let (updated, changed) = strip_moon_home_block(&existing);
    if !changed {
        report.detail(format!("shell.zprofile.clean={}", zprofile_path.display()));
        return Ok(());
    }
    if opts.dry_run {
        report.detail(format!(
            "shell.zprofile.update=planned path={}",
            zprofile_path.display()
        ));
        return Ok(());
    }

    fs::write(&zprofile_path, updated)
        .with_context(|| format!("failed to write {}", zprofile_path.display()))?;
    report.detail(format!(
        "shell.zprofile.cleaned={}",
        zprofile_path.display()
    ));
    Ok(())
}

fn strip_moon_home_block(existing: &str) -> (String, bool) {
    let lines = existing.lines().collect::<Vec<_>>();
    let mut out = Vec::new();
    let mut idx = 0usize;
    let mut removed = false;

    while idx < lines.len() {
        if lines[idx].trim() == "# Moon runtime home" {
            removed = true;
            idx += 1;
            if idx < lines.len() {
                let trimmed = lines[idx].trim_start();
                if trimmed.starts_with("export MOON_HOME=") || trimmed.starts_with("MOON_HOME=") {
                    idx += 1;
                }
            }
            if idx < lines.len() && lines[idx].trim().is_empty() {
                idx += 1;
            }
            continue;
        }
        out.push(lines[idx]);
        idx += 1;
    }

    let mut updated = out.join("\n");
    if existing.ends_with('\n') && !updated.is_empty() {
        updated.push('\n');
    }
    (updated, removed)
}

fn maybe_remove_binary(
    current_exe: &Path,
    opts: &MoonUninstallOptions,
    report: &mut CommandReport,
) -> Result<()> {
    if !opts.remove_binary {
        report.detail("binary.remove=skipped reason=flag_not_set".to_string());
        return Ok(());
    }
    if is_dev_build_path(current_exe) {
        report.detail(format!(
            "binary.remove=skipped reason=development_binary path={}",
            current_exe.display()
        ));
        return Ok(());
    }
    if opts.dry_run {
        report.detail("binary.remove=planned via `cargo uninstall moon`".to_string());
        return Ok(());
    }

    let cargo = match which::which("cargo") {
        Ok(path) => path,
        Err(_) => {
            report.issue("binary.remove=failed (cargo unavailable)".to_string());
            return Ok(());
        }
    };
    let output = Command::new(cargo)
        .args(["uninstall", "moon"])
        .output()
        .context("failed to run `cargo uninstall moon`")?;
    if output.status.success() {
        report.detail("binary.remove=ok via cargo uninstall".to_string());
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("is not installed") || lower.contains("not installed package") {
        report.detail("binary.remove=skipped reason=not_installed".to_string());
        return Ok(());
    }
    report.issue(format!("binary.remove=failed ({stderr})"));
    Ok(())
}

fn is_dev_build_path(path: &Path) -> bool {
    let rendered = path.to_string_lossy();
    rendered.contains("/target/debug/") || rendered.contains("/target/release/")
}

fn remove_launchd_artifacts(opts: &MoonUninstallOptions, report: &mut CommandReport) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let plist_path = crate::moon::launchd::plist_path()?;
        if plist_path.exists() {
            if opts.dry_run {
                report.detail(format!(
                    "launchd.remove=planned path={}",
                    plist_path.display()
                ));
            } else {
                let bootout_out = crate::moon::launchd::bootout_service()?;
                if bootout_out.status.success() {
                    report.detail("launchd.bootout=ok".to_string());
                } else {
                    report.detail(format!(
                        "launchd.bootout=ignored ({})",
                        crate::moon::launchd::summarize_command_failure(&bootout_out)
                    ));
                }
                fs::remove_file(&plist_path)
                    .with_context(|| format!("failed to remove {}", plist_path.display()))?;
                report.detail(format!("launchd.plist.removed={}", plist_path.display()));
            }
        } else {
            report.detail(format!(
                "launchd.remove=skipped reason=missing path={}",
                plist_path.display()
            ));
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = opts;
        report.detail("launchd.remove=skipped reason=unsupported_platform".to_string());
    }

    Ok(())
}

fn remove_path_entry(
    path: &Path,
    opts: &MoonUninstallOptions,
    report: &mut CommandReport,
    label: &str,
) -> Result<()> {
    if !path.exists() {
        report.detail(format!(
            "{label}=skipped reason=missing path={}",
            path.display()
        ));
        return Ok(());
    }
    if opts.dry_run {
        report.detail(format!("{label}=planned path={}", path.display()));
        return Ok(());
    }
    if path.is_dir() {
        fs::remove_dir_all(path).with_context(|| format!("failed to remove {}", path.display()))?;
    } else {
        fs::remove_file(path).with_context(|| format!("failed to remove {}", path.display()))?;
    }
    report.detail(format!("{label}=removed path={}", path.display()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::strip_moon_home_block;

    #[test]
    fn strip_moon_home_block_removes_install_managed_block() {
        let input =
            "line1\n# Moon runtime home\nexport MOON_HOME=\"${MOON_HOME:-$HOME/.moon}\"\n\nline2\n";
        let (updated, changed) = strip_moon_home_block(input);
        assert!(changed);
        assert_eq!(updated, "line1\nline2\n");
    }
}
