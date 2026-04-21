use anyhow::{Context, Result};
use std::env;
use std::fs;
#[cfg(target_os = "macos")]
use std::io::ErrorKind;
#[cfg(target_os = "macos")]
use std::path::Path;
#[cfg(target_os = "macos")]
use std::process::Command;

use crate::assets::{write_runtime_docs, write_runtime_skills};
use crate::commands::CommandReport;
use crate::commands::moon_stop;
use crate::moon::config::load_context_policy_if_explicit_env;
use crate::moon::state::state_file_path;
use crate::openclaw::config::{
    ConfigPatchOptions, apply_config_patches, ensure_moon_owned_memory_contract,
    ensure_plugin_enabled, ensure_plugin_install_record, ensure_plugin_runtime_config,
    ensure_plugin_slot, read_config_value, write_config_atomic,
};
use crate::openclaw::paths::resolve_paths;
use crate::openclaw::plugin_install;

#[derive(Debug, Clone)]
pub struct InstallOptions {
    pub force: bool,
    pub dry_run: bool,
    pub apply: bool,
}

const DEFAULT_RUNTIME_ENV_TEMPLATE: &str = "\
# MOON runtime environment
# Loaded by moon from $MOON_HOME/.env
#
# Minimal cleanse profile (required for remote compaction):
# MOON_CLEANSE_PROVIDER=gemini
# MOON_CLEANSE_MODEL=gemini-3.1-flash-lite-preview
# GEMINI_API_KEY=...
#
# Optional synthesis profile:
# MOON_WISDOM_PROVIDER=openai
# MOON_WISDOM_MODEL=gpt-4.1
# OPENAI_API_KEY=...
#
# Optional managed OpenAI Codex OAuth profile:
# MOON_CLEANSE_PROVIDER=openai-codex
# MOON_CLEANSE_MODEL=gpt-5.4
# moon login
# OPENAI_CODEX_BASE_URL=https://chatgpt.com/backend-api
# Optional manual override instead of `moon login`
# OPENAI_OAUTH_TOKEN=...
#
# Optional path overrides:
# MOON_MDS_DIR=$MOON_HOME/mds
# MOON_MLIB_DIR=$MOON_HOME/mlib
";

pub fn run(opts: &InstallOptions) -> Result<CommandReport> {
    let paths = resolve_paths()?;
    let moon_paths = crate::moon::paths::resolve_paths()?;
    let current_exe = env::current_exe().context("failed to resolve current executable path")?;
    let mut report = CommandReport::new("install");

    report.detail("runtime.controller=moon-context-engine".to_string());
    report.detail("runtime.watcher_role=transitional-shell".to_string());
    report.detail(format!("runtime.root={}", moon_paths.moon_home.display()));
    report.detail(format!("runtime.moon_path={}", current_exe.display()));
    report.detail(format!("runtime.context_engine_slot={}", paths.plugin_id));

    report.detail("preflight: stopping transitional watcher daemon and clearing lock".to_string());
    report.merge(moon_stop::run()?);

    let plugin = plugin_install::install_plugin(&paths, opts.dry_run)?;
    report.detail(format!("plugin_dir={}", plugin.path));
    report.detail(format!("plugin_changed={}", plugin.changed));

    let mut cfg = read_config_value(&paths)?;
    let context_policy = load_context_policy_if_explicit_env()?;
    if let Some(policy) = &context_policy {
        report.detail(format!(
            "context.policy=window_mode={:?} compaction_authority={:?}",
            policy.window_mode, policy.compaction_authority
        ));
    } else {
        report.detail(
            "context.policy=default (no explicit MOON_CONFIG_PATH/MOON_HOME context section)"
                .to_string(),
        );
    }

    let patch = apply_config_patches(
        &mut cfg,
        &ConfigPatchOptions { force: opts.force },
        &paths.plugin_id,
        context_policy.as_ref(),
    );

    let plugin_patch = ensure_plugin_enabled(&mut cfg, &paths.plugin_id);
    let install_record_patch =
        ensure_plugin_install_record(&mut cfg, &paths.plugin_id, &paths.plugin_dir);
    let slot_patch = ensure_plugin_slot(&mut cfg, "contextEngine", &paths.plugin_id);
    let runtime_patch = ensure_plugin_runtime_config(
        &mut cfg,
        &paths.plugin_id,
        &current_exe,
        &moon_paths.moon_home,
    );
    let memory_contract_patch = ensure_moon_owned_memory_contract(&mut cfg);

    for key in patch.inserted_paths {
        report.detail(format!("inserted {key}"));
    }
    for key in patch.forced_paths {
        report.detail(format!("forced {key}"));
    }
    for key in patch.removed_paths {
        report.detail(format!("removed {key}"));
    }
    for key in plugin_patch.inserted_paths {
        report.detail(format!("inserted {key}"));
    }
    for key in plugin_patch.forced_paths {
        report.detail(format!("forced {key}"));
    }
    for key in install_record_patch.inserted_paths {
        report.detail(format!("inserted {key}"));
    }
    for key in install_record_patch.forced_paths {
        report.detail(format!("forced {key}"));
    }
    for key in slot_patch.inserted_paths {
        report.detail(format!("inserted {key}"));
    }
    for key in slot_patch.forced_paths {
        report.detail(format!("forced {key}"));
    }
    for key in runtime_patch.inserted_paths {
        report.detail(format!("inserted {key}"));
    }
    for key in runtime_patch.forced_paths {
        report.detail(format!("forced {key}"));
    }
    for key in memory_contract_patch.inserted_paths {
        report.detail(format!("inserted {key}"));
    }
    for key in memory_contract_patch.forced_paths {
        report.detail(format!("forced {key}"));
    }

    let changed = patch.changed
        || plugin_patch.changed
        || install_record_patch.changed
        || slot_patch.changed
        || runtime_patch.changed
        || memory_contract_patch.changed
        || plugin.changed;
    if changed && opts.apply && !opts.dry_run {
        let path_written = write_config_atomic(&paths, &cfg)?;
        report.detail(format!("updated config: {path_written}"));
    } else if changed && (opts.dry_run || !opts.apply) {
        report.detail("config changes planned but not applied".to_string());
    } else {
        report.detail("config already satisfied".to_string());
    }

    ensure_runtime_root_layout(&moon_paths, opts, &mut report)?;
    ensure_runtime_docs_and_skills(&paths, &moon_paths, opts, &mut report)?;
    if let Err(err) = ensure_shell_profile_moon_home(&moon_paths, opts, &mut report) {
        report.issue(format!("shell profile setup failed: {err:#}"));
    }

    if let Err(err) = ensure_default_autostart(opts, &mut report) {
        report.issue(format!("autostart setup failed: {err:#}"));
    }

    if opts.apply && !opts.dry_run {
        crate::commands::align_openclaw_plugin_state(&mut report, "install");
    }

    Ok(report)
}

fn ensure_shell_profile_moon_home(
    moon_paths: &crate::moon::paths::MoonPaths,
    opts: &InstallOptions,
    report: &mut CommandReport,
) -> Result<()> {
    let Some(home_dir) = dirs::home_dir() else {
        report.detail("shell.zprofile=skipped reason=home_dir_unavailable".to_string());
        return Ok(());
    };

    let zprofile_path = home_dir.join(".zprofile");
    let existing = match fs::read_to_string(&zprofile_path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to read {}", zprofile_path.display()));
        }
    };

    let has_moon_home = existing.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("export MOON_HOME=") || trimmed.starts_with("MOON_HOME=")
    });
    if has_moon_home {
        report.detail(format!("shell.zprofile.ready={}", zprofile_path.display()));
        return Ok(());
    }

    let default_moon_home = if moon_paths.moon_home == home_dir.join(".moon") {
        "$HOME/.moon".to_string()
    } else {
        shell_escape_double_quoted(&moon_paths.moon_home.display().to_string())
    };
    let export_line = format!("export MOON_HOME=\"${{MOON_HOME:-{default_moon_home}}}\"");

    if opts.dry_run {
        report.detail(format!(
            "shell.plan.append={} line={}",
            zprofile_path.display(),
            export_line
        ));
        return Ok(());
    }

    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    if !updated.is_empty() {
        updated.push('\n');
    }
    updated.push_str("# Moon runtime home\n");
    updated.push_str(&export_line);
    updated.push('\n');

    fs::write(&zprofile_path, updated)
        .with_context(|| format!("failed to write {}", zprofile_path.display()))?;
    report.detail(format!(
        "shell.zprofile.updated={}",
        zprofile_path.display()
    ));
    Ok(())
}

fn shell_escape_double_quoted(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$")
        .replace('`', "\\`")
}

fn ensure_runtime_root_layout(
    paths: &crate::moon::paths::MoonPaths,
    opts: &InstallOptions,
    report: &mut CommandReport,
) -> Result<()> {
    let state_dir = state_file_path(paths)
        .parent()
        .map(|path| path.to_path_buf())
        .unwrap_or_else(|| paths.moon_home.join("state"));

    let directories = [
        paths.moon_home.clone(),
        paths.raw_dir.clone(),
        paths.mds_dir.clone(),
        paths.mlib_dir.clone(),
        paths.cleanse_dir.clone(),
        paths.memory_dir.clone(),
        paths.logs_dir.clone(),
        paths.context_engine_dir.clone(),
        paths.qmd_config_dir.clone(),
        state_dir,
    ];

    let qmd_db_dir = paths
        .qmd_db
        .parent()
        .map(|path| path.to_path_buf())
        .unwrap_or_else(|| paths.moon_home.join("qmd"));

    if opts.dry_run {
        for dir in &directories {
            report.detail(format!("runtime.plan.mkdir={}", dir.display()));
        }
        report.detail(format!("runtime.plan.mkdir={}", qmd_db_dir.display()));
        let runtime_env_path = paths.moon_home.join(".env");
        if !runtime_env_path.exists() {
            report.detail(format!("runtime.plan.touch={}", runtime_env_path.display()));
        }
        if !paths.memory_file.exists() {
            report.detail(format!(
                "runtime.plan.touch={}",
                paths.memory_file.display()
            ));
        }
        report.detail("runtime.bootstrap=dry-run".to_string());
        return Ok(());
    }

    for dir in &directories {
        fs::create_dir_all(dir)
            .with_context(|| format!("failed to create runtime dir {}", dir.display()))?;
        report.detail(format!("runtime.dir.ready={}", dir.display()));
    }
    fs::create_dir_all(&qmd_db_dir)
        .with_context(|| format!("failed to create runtime dir {}", qmd_db_dir.display()))?;
    report.detail(format!("runtime.dir.ready={}", qmd_db_dir.display()));

    crate::moon::fs_security::ensure_private_dir(&paths.logs_dir)?;

    if !paths.memory_file.exists() {
        fs::write(&paths.memory_file, "# MOON Memory\n")
            .with_context(|| format!("failed to write {}", paths.memory_file.display()))?;
        report.detail(format!(
            "runtime.file.created={}",
            paths.memory_file.display()
        ));
    } else {
        report.detail(format!(
            "runtime.file.ready={}",
            paths.memory_file.display()
        ));
    }

    let runtime_env_path = paths.moon_home.join(".env");
    if crate::moon::fs_security::ensure_private_file_with_contents_if_missing(
        &runtime_env_path,
        DEFAULT_RUNTIME_ENV_TEMPLATE.as_bytes(),
    )? {
        report.detail(format!(
            "runtime.env.created={}",
            runtime_env_path.display()
        ));
    } else {
        report.detail(format!("runtime.env.ready={}", runtime_env_path.display()));
    }

    harden_runtime_secret_artifacts(paths)?;
    report.detail("security.runtime_secret_permissions=owner-only".to_string());

    report.detail("runtime.bootstrap=ready".to_string());
    Ok(())
}

fn harden_runtime_secret_artifacts(paths: &crate::moon::paths::MoonPaths) -> Result<()> {
    let runtime_env_path = paths.moon_home.join(".env");
    crate::moon::fs_security::harden_private_file_if_exists(&runtime_env_path)?;
    crate::moon::fs_security::harden_private_dir_if_exists(&paths.logs_dir)?;
    crate::moon::fs_security::harden_private_file_if_exists(&paths.logs_dir.join("audit.log"))?;
    crate::moon::fs_security::harden_private_file_if_exists(
        &paths.logs_dir.join("distill.audit.log"),
    )?;

    let auth_dir = paths.moon_home.join("auth");
    crate::moon::fs_security::harden_private_dir_if_exists(&auth_dir)?;
    crate::moon::fs_security::harden_private_file_if_exists(&auth_dir.join("openai-codex.json"))?;
    crate::moon::fs_security::harden_private_file_if_exists(&auth_dir.join("openai-codex.lock"))?;
    Ok(())
}

fn ensure_runtime_docs_and_skills(
    openclaw_paths: &crate::openclaw::paths::OpenClawPaths,
    moon_paths: &crate::moon::paths::MoonPaths,
    opts: &InstallOptions,
    report: &mut CommandReport,
) -> Result<()> {
    let skills_root = openclaw_paths.state_dir.join("skills");
    let legacy_bootstrap_path = moon_paths.moon_home.join("BOOTSTRAP.md");

    if opts.dry_run {
        for (rel, _) in crate::assets::runtime_doc_asset_contents() {
            report.detail(format!(
                "runtime.plan.write={}",
                moon_paths.moon_home.join(rel).display()
            ));
        }
        if legacy_bootstrap_path.exists() {
            report.detail(format!(
                "runtime.plan.remove={}",
                legacy_bootstrap_path.display()
            ));
        }
        for (rel, _) in crate::assets::runtime_skill_asset_contents() {
            report.detail(format!(
                "runtime.plan.write={}",
                skills_root.join(rel).display()
            ));
        }
        return Ok(());
    }

    write_runtime_docs(&moon_paths.moon_home).with_context(|| {
        format!(
            "failed to export installed-runtime docs into {}",
            moon_paths.moon_home.display()
        )
    })?;
    for (rel, _) in crate::assets::runtime_doc_asset_contents() {
        report.detail(format!(
            "runtime.doc.ready={}",
            moon_paths.moon_home.join(rel).display()
        ));
    }
    match fs::remove_file(&legacy_bootstrap_path) {
        Ok(()) => report.detail(format!(
            "runtime.doc.removed_legacy={}",
            legacy_bootstrap_path.display()
        )),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(err).with_context(|| {
                format!(
                    "failed to remove legacy runtime bootstrap doc {}",
                    legacy_bootstrap_path.display()
                )
            });
        }
    }

    write_runtime_skills(&skills_root).with_context(|| {
        format!(
            "failed to export runtime skills into {}",
            skills_root.display()
        )
    })?;
    for (rel, _) in crate::assets::runtime_skill_asset_contents() {
        report.detail(format!(
            "runtime.skill.ready={}",
            skills_root.join(rel).display()
        ));
    }

    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn ensure_default_autostart(opts: &InstallOptions, report: &mut CommandReport) -> Result<()> {
    let _ = opts;
    report.detail("autostart=skipped reason=unsupported_platform".to_string());
    Ok(())
}

#[cfg(target_os = "macos")]
const LAUNCHD_LABEL: &str = "com.moon.watch";
#[cfg(target_os = "macos")]
const LAUNCHD_PLIST_NAME: &str = "com.moon.watch.plist";

#[cfg(target_os = "macos")]
fn ensure_default_autostart(opts: &InstallOptions, report: &mut CommandReport) -> Result<()> {
    let current_exe = env::current_exe().context("failed to resolve current executable path")?;
    report.detail(format!("autostart.provider=launchd label={LAUNCHD_LABEL}"));

    if is_dev_build_path(&current_exe) {
        report.detail(format!(
            "autostart.launchd=skipped reason=development_binary path={}",
            current_exe.display()
        ));
        report.detail(
            "autostart.hint=run `cargo install --path .` then rerun `moon install` from installed binary"
                .to_string(),
        );
        return Ok(());
    }

    let moon_paths = crate::moon::paths::resolve_paths()?;
    let home_dir = dirs::home_dir().context("HOME directory could not be resolved")?;
    let launch_agents_dir = home_dir.join("Library").join("LaunchAgents");
    let plist_path = launch_agents_dir.join(LAUNCHD_PLIST_NAME);
    let stdout_path = moon_paths.logs_dir.join("launchd.stdout.log");
    let stderr_path = moon_paths.logs_dir.join("launchd.stderr.log");
    let moon_config_path = crate::moon::config::resolve_config_path();
    let path_value = default_launchd_path(&home_dir, current_exe.parent());
    let plist_payload = render_launchd_plist(
        LAUNCHD_LABEL,
        &current_exe,
        &moon_paths.moon_home,
        &moon_paths.moon_home,
        &moon_paths.logs_dir,
        &stdout_path,
        &stderr_path,
        &home_dir,
        &path_value,
        moon_config_path.as_deref(),
    );

    report.detail(format!(
        "autostart.launchd.binary={}",
        current_exe.display()
    ));
    report.detail(format!(
        "autostart.launchd.working_dir={}",
        moon_paths.moon_home.display()
    ));
    report.detail(format!("autostart.launchd.plist={}", plist_path.display()));
    if opts.dry_run {
        report.detail("autostart.launchd.mode=dry-run (no launchctl changes)".to_string());
        return Ok(());
    }

    fs::create_dir_all(&launch_agents_dir)
        .with_context(|| format!("failed to create {}", launch_agents_dir.display()))?;
    fs::create_dir_all(&moon_paths.logs_dir)
        .with_context(|| format!("failed to create {}", moon_paths.logs_dir.display()))?;

    let mut previous_working_dir = None::<String>;
    let plist_changed = match fs::read_to_string(&plist_path) {
        Ok(existing) => {
            previous_working_dir = extract_launchd_working_directory(&existing);
            existing != plist_payload
        }
        Err(err) if err.kind() == ErrorKind::NotFound => true,
        Err(err) => {
            return Err(err).with_context(|| format!("failed to read {}", plist_path.display()));
        }
    };
    if plist_changed {
        fs::write(&plist_path, plist_payload)
            .with_context(|| format!("failed to write {}", plist_path.display()))?;
    }
    report.detail(format!("autostart.launchd.plist_changed={plist_changed}"));
    if let Some(previous) = previous_working_dir {
        let expected = moon_paths.moon_home.display().to_string();
        if previous != expected {
            report.detail(format!(
                "autostart.launchd.repair=working_directory_wrong previous={} fixed={}",
                previous, expected
            ));
        }
    }
    reset_launchd_stream_logs(&stdout_path, &stderr_path, report)?;

    let uid = resolve_uid()?;
    let domain = format!("gui/{uid}");
    let plist_arg = plist_path.display().to_string();
    let bootout_out = run_launchctl(["bootout", &domain, &plist_arg].as_slice())?;
    if bootout_out.status.success() {
        report.detail("autostart.launchd.bootout=ok".to_string());
    } else {
        report.detail(format!(
            "autostart.launchd.bootout=ignored ({})",
            summarize_command_failure(&bootout_out)
        ));
    }

    let bootstrap_out = run_launchctl(["bootstrap", &domain, &plist_arg].as_slice())?;
    if !bootstrap_out.status.success() {
        anyhow::bail!(
            "launchctl bootstrap failed: {}",
            summarize_command_failure(&bootstrap_out)
        );
    }
    report.detail("autostart.launchd.bootstrap=ok".to_string());

    let target = format!("{domain}/{LAUNCHD_LABEL}");
    let kickstart_out = run_launchctl(["kickstart", "-k", &target].as_slice())?;
    if !kickstart_out.status.success() {
        anyhow::bail!(
            "launchctl kickstart failed: {}",
            summarize_command_failure(&kickstart_out)
        );
    }
    report.detail("autostart.launchd.kickstart=ok".to_string());
    report.detail("autostart.launchd.enabled=true".to_string());
    Ok(())
}

#[cfg(target_os = "macos")]
fn run_launchctl(args: &[&str]) -> Result<std::process::Output> {
    Command::new("launchctl")
        .args(args)
        .output()
        .with_context(|| format!("failed to execute launchctl {}", args.join(" ")))
}

#[cfg(target_os = "macos")]
fn summarize_command_failure(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stderr.is_empty() {
        return stderr;
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !stdout.is_empty() {
        return stdout;
    }
    match output.status.code() {
        Some(code) => format!("exit code {code}"),
        None => "terminated by signal".to_string(),
    }
}

#[cfg(target_os = "macos")]
fn reset_launchd_stream_logs(
    stdout_path: &Path,
    stderr_path: &Path,
    report: &mut CommandReport,
) -> Result<()> {
    for path in [stdout_path, stderr_path] {
        fs::write(path, b"").with_context(|| format!("failed to reset {}", path.display()))?;
        report.detail(format!("autostart.launchd.log_reset={}", path.display()));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn resolve_uid() -> Result<String> {
    let output = Command::new("id")
        .arg("-u")
        .output()
        .context("failed to resolve user id via `id -u`")?;
    if !output.status.success() {
        anyhow::bail!("`id -u` failed: {}", summarize_command_failure(&output));
    }

    let uid = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if uid.is_empty() {
        anyhow::bail!("`id -u` returned empty output");
    }
    Ok(uid)
}

#[cfg(target_os = "macos")]
fn is_dev_build_path(path: &Path) -> bool {
    let normalized = path.display().to_string();
    normalized.contains("target/debug")
        || normalized.contains("target/release")
        || normalized.contains("target\\debug")
        || normalized.contains("target\\release")
}

#[cfg(target_os = "macos")]
fn default_launchd_path(home_dir: &Path, binary_parent: Option<&Path>) -> String {
    let mut parts = Vec::new();

    if let Some(parent) = binary_parent {
        push_unique_path_entry(&mut parts, parent.display().to_string());
    }

    for entry in [
        "/opt/homebrew/bin".to_string(),
        "/usr/local/bin".to_string(),
        "/usr/bin".to_string(),
        "/bin".to_string(),
        "/usr/sbin".to_string(),
        "/sbin".to_string(),
        home_dir.join(".cargo/bin").display().to_string(),
        home_dir.join(".bun/bin").display().to_string(),
        home_dir.join(".local/bin").display().to_string(),
    ] {
        push_unique_path_entry(&mut parts, entry);
    }

    parts.join(":")
}

#[cfg(target_os = "macos")]
fn push_unique_path_entry(parts: &mut Vec<String>, entry: String) {
    if !parts.iter().any(|existing| existing == &entry) {
        parts.push(entry);
    }
}

#[cfg(target_os = "macos")]
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(target_os = "macos")]
#[allow(clippy::too_many_arguments)]
fn render_launchd_plist(
    label: &str,
    binary_path: &Path,
    working_dir: &Path,
    moon_home: &Path,
    moon_logs_dir: &Path,
    stdout_path: &Path,
    stderr_path: &Path,
    home_dir: &Path,
    path_value: &str,
    moon_config_path: Option<&Path>,
) -> String {
    let config_entry = moon_config_path.map_or_else(String::new, |path| {
        format!(
            "    <key>MOON_CONFIG_PATH</key><string>{}</string>\n",
            xml_escape(&path.display().to_string())
        )
    });

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{}</string>
    <string>watch</string>
    <string>--daemon</string>
  </array>
  <key>WorkingDirectory</key><string>{}</string>
  <key>EnvironmentVariables</key>
  <dict>
    <key>HOME</key><string>{}</string>
    <key>PATH</key><string>{}</string>
    <key>MOON_HOME</key><string>{}</string>
    <key>MOON_LOGS_DIR</key><string>{}</string>
{}
  </dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>{}</string>
  <key>StandardErrorPath</key><string>{}</string>
</dict>
</plist>
"#,
        xml_escape(label),
        xml_escape(&binary_path.display().to_string()),
        xml_escape(&working_dir.display().to_string()),
        xml_escape(&home_dir.display().to_string()),
        xml_escape(path_value),
        xml_escape(&moon_home.display().to_string()),
        xml_escape(&moon_logs_dir.display().to_string()),
        config_entry,
        xml_escape(&stdout_path.display().to_string()),
        xml_escape(&stderr_path.display().to_string()),
    )
}

#[cfg(target_os = "macos")]
fn extract_launchd_working_directory(plist: &str) -> Option<String> {
    let marker = "<key>WorkingDirectory</key><string>";
    let start = plist.find(marker)? + marker.len();
    let end = plist[start..].find("</string>")?;
    Some(plist[start..start + end].to_string())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::{extract_launchd_working_directory, render_launchd_plist};
    use std::path::Path;

    #[test]
    fn launchd_plist_uses_moon_home_as_working_directory() {
        let plist = render_launchd_plist(
            "com.moon.watch",
            Path::new("/Users/test/.cargo/bin/moon"),
            Path::new("/Users/test/.moon"),
            Path::new("/Users/test/.moon"),
            Path::new("/Users/test/.moon/logs"),
            Path::new("/Users/test/.moon/logs/launchd.stdout.log"),
            Path::new("/Users/test/.moon/logs/launchd.stderr.log"),
            Path::new("/Users/test"),
            "/Users/test/.cargo/bin:/opt/homebrew/bin:/usr/bin:/bin",
            Some(Path::new("/Users/test/.moon/moon.toml")),
        );

        assert!(plist.contains("<key>WorkingDirectory</key><string>/Users/test/.moon</string>"));
    }

    #[test]
    fn extract_launchd_working_directory_reads_plist_value() {
        let plist = render_launchd_plist(
            "com.moon.watch",
            Path::new("/Users/test/.cargo/bin/moon"),
            Path::new("/Users/test/.moon"),
            Path::new("/Users/test/.moon"),
            Path::new("/Users/test/.moon/logs"),
            Path::new("/Users/test/.moon/logs/launchd.stdout.log"),
            Path::new("/Users/test/.moon/logs/launchd.stderr.log"),
            Path::new("/Users/test"),
            "/Users/test/.cargo/bin:/opt/homebrew/bin:/usr/bin:/bin",
            Some(Path::new("/Users/test/.moon/moon.toml")),
        );

        assert_eq!(
            extract_launchd_working_directory(&plist).as_deref(),
            Some("/Users/test/.moon")
        );
    }
}
