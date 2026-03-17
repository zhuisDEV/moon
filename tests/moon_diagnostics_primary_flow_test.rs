use predicates::str::contains;
use serde_json::json;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use tempfile::tempdir;

fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_secs()
}

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("mkdir parent");
    }
    fs::write(path, content).expect("write file");
}

fn write_executable(path: &Path, content: &str) {
    write_file(path, content);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod");
    }
}

fn write_moon_config(moon_home: &Path, lifecycle_mode: &str, lifecycle_command_mode: Option<&str>) {
    let mut config = format!("[hot_collection]\nlifecycle_mode = \"{lifecycle_mode}\"\n");
    if let Some(command_mode) = lifecycle_command_mode {
        config.push_str(&format!("lifecycle_command_mode = \"{command_mode}\"\n"));
    }
    write_file(&moon_home.join("moon.toml"), &config);
}

fn setup_runtime_tree(root: &Path) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let moon_home = root.join("moon");
    let sessions_dir = root.join("sessions");
    let qmd_bin = root.join("bin/qmd");

    for dir in [
        moon_home.join("raw"),
        moon_home.join("mds"),
        moon_home.join("mlib"),
        moon_home.join("cleanse"),
        moon_home.join("archives"),
        moon_home.join("memory"),
        moon_home.join("logs"),
        moon_home.join("mce"),
        sessions_dir.clone(),
    ] {
        fs::create_dir_all(dir).expect("mkdir runtime dir");
    }

    write_file(&moon_home.join("MEMORY.md"), "# Memory\n");
    write_file(&moon_home.join(".env"), "\n");
    write_executable(
        &qmd_bin,
        r#"#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == "collection" && "${2:-}" == "--help" ]]; then
  echo "Commands: add remove show"
  exit 0
fi
exit 0
"#,
    );

    (moon_home, sessions_dir, qmd_bin)
}

#[test]
fn status_reports_latest_assembly_artifact_from_state() {
    let tmp = tempdir().expect("tempdir");
    let (moon_home, sessions_dir, qmd_bin) = setup_runtime_tree(tmp.path());
    let moon_home = fs::canonicalize(&moon_home).expect("canonicalize moon");
    let sessions_dir = fs::canonicalize(&sessions_dir).expect("canonicalize sessions");
    let qmd_bin = fs::canonicalize(&qmd_bin).expect("canonicalize qmd");

    let session_id = "session-status";
    let assembly_path = moon_home.join("mce").join(format!("{session_id}.md"));
    write_file(&assembly_path, "# MOON Assembly Context\n");
    write_file(
        &moon_home.join("state/moon_state.json"),
        &format!(
            "{}\n",
            serde_json::to_string_pretty(&json!({
                "schema_version": 3,
                "last_heartbeat_epoch_secs": now_epoch_secs(),
                "last_session_id": session_id,
                "last_assembly_session_id": session_id,
                "last_assembly_epoch_secs": now_epoch_secs(),
                "distilled_archives": {},
                "embedded_projections": {},
                "inbound_seen_files": {}
            }))
            .expect("serialize state")
        ),
    );

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("OPENCLAW_SESSIONS_DIR", &sessions_dir)
        .env("QMD_BIN", &qmd_bin)
        .arg("status")
        .assert()
        .success()
        .stdout(contains(format!(
            "state.last_assembly_session_id={session_id}"
        )))
        .stdout(contains(format!(
            "context_engine.latest_output_path={}",
            assembly_path.display()
        )))
        .stdout(contains("context_engine.latest_output_exists=true"))
        .stdout(contains("hot_collection.lifecycle_mode=degrade"))
        .stdout(contains("hot_collection.lifecycle_command_mode=primary"))
        .stdout(contains("hot_collection.lifecycle_capability=primary"));
}

#[test]
fn health_fails_when_recorded_latest_assembly_output_is_missing() {
    let tmp = tempdir().expect("tempdir");
    let (moon_home, _sessions_dir, _qmd_bin) = setup_runtime_tree(tmp.path());
    let moon_home = fs::canonicalize(&moon_home).expect("canonicalize moon");

    let session_id = "session-missing";
    let expected_output = moon_home.join("mce").join(format!("{session_id}.md"));
    write_file(
        &moon_home.join("state/moon_state.json"),
        &format!(
            "{}\n",
            serde_json::to_string_pretty(&json!({
                "schema_version": 3,
                "last_heartbeat_epoch_secs": now_epoch_secs(),
                "last_session_id": session_id,
                "last_assembly_session_id": session_id,
                "last_assembly_epoch_secs": now_epoch_secs(),
                "distilled_archives": {},
                "embedded_projections": {},
                "inbound_seen_files": {}
            }))
            .expect("serialize state")
        ),
    );

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .arg("health")
        .assert()
        .failure()
        .stdout(contains(format!(
            "context_engine.latest_output=missing ({})",
            expected_output.display()
        )));
}

#[test]
fn health_fails_when_strict_mode_requires_missing_lifecycle_support() {
    let tmp = tempdir().expect("tempdir");
    let (moon_home, _sessions_dir, qmd_bin) = setup_runtime_tree(tmp.path());
    let moon_home = fs::canonicalize(&moon_home).expect("canonicalize moon");
    let qmd_bin = fs::canonicalize(&qmd_bin).expect("canonicalize qmd");
    write_moon_config(&moon_home, "strict", None);
    write_executable(
        &qmd_bin,
        r#"#!/usr/bin/env bash
set -euo pipefail
echo "unknown command" >&2
exit 1
"#,
    );

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("QMD_BIN", &qmd_bin)
        .arg("health")
        .assert()
        .failure()
        .stdout(contains(
            "hot collection lifecycle strict mode requires qmd lifecycle support",
        ));
}
