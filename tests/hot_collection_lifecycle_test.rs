#![cfg(not(windows))]
use predicates::str::contains;
use std::fs;
use std::path::Path;
use tempfile::tempdir;

fn write_executable(path: &Path, script: &str) {
    fs::write(path, script).expect("write script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod");
    }
}

fn write_moon_env(moon_home: &Path) {
    fs::create_dir_all(moon_home).expect("mkdir moon env root");
    fs::write(moon_home.join(".env"), "\n").expect("write moon .env");
}

fn write_moon_config(moon_home: &Path, lifecycle_mode: &str, lifecycle_command_mode: Option<&str>) {
    let mut config = format!("[hot_collection]\nlifecycle_mode = \"{lifecycle_mode}\"\n");
    if let Some(command_mode) = lifecycle_command_mode {
        config.push_str(&format!("lifecycle_command_mode = \"{command_mode}\"\n"));
    }
    fs::write(moon_home.join("moon.toml"), config).expect("write moon.toml");
}

fn write_source(path: &Path, text: &str) {
    fs::write(
        path,
        format!(
            "{{\"message\":{{\"role\":\"user\",\"content\":[{{\"type\":\"text\",\"text\":\"{}\"}}]}}}}\n",
            text
        ),
    )
    .expect("write source");
}

#[test]
fn context_engine_strict_mode_fails_when_qmd_binary_missing() {
    let tmp = tempdir().expect("tempdir");
    let moon_home = tmp.path().join("moon");
    fs::create_dir_all(&moon_home).expect("mkdir moon");
    let moon_home = fs::canonicalize(&moon_home).expect("canonicalize moon");
    write_moon_env(&moon_home);
    write_moon_config(&moon_home, "strict", None);

    let source = moon_home.join("session-a.jsonl");
    write_source(&source, "session a");

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(&moon_home)
        .env("MOON_HOME", &moon_home)
        .env("QMD_BIN", "/definitely/not/a/qmd")
        .arg("context-engine")
        .args(["--source", &source.display().to_string()])
        .args(["--session-id", "strict-a"])
        .args(["--used-tokens", "1000"])
        .args(["--max-tokens", "200000"])
        .assert()
        .failure()
        .stderr(contains("hot collection lifecycle failed in strict mode"));
}

#[test]
fn context_engine_strict_mode_fails_when_lifecycle_commands_unsupported() {
    let tmp = tempdir().expect("tempdir");
    let moon_home = tmp.path().join("moon");
    fs::create_dir_all(&moon_home).expect("mkdir moon");
    let moon_home = fs::canonicalize(&moon_home).expect("canonicalize moon");
    write_moon_env(&moon_home);
    write_moon_config(&moon_home, "strict", None);

    let source = moon_home.join("session-a.jsonl");
    write_source(&source, "session a");

    let qmd = tmp.path().join("qmd");
    write_executable(
        &qmd,
        r#"#!/usr/bin/env bash
set -euo pipefail
echo "unknown command" >&2
exit 1
"#,
    );

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(&moon_home)
        .env("MOON_HOME", &moon_home)
        .env("QMD_BIN", &qmd)
        .arg("context-engine")
        .args(["--source", &source.display().to_string()])
        .args(["--session-id", "strict-b"])
        .args(["--used-tokens", "1000"])
        .args(["--max-tokens", "200000"])
        .assert()
        .failure()
        .stderr(contains("hot collection lifecycle failed in strict mode"))
        .stderr(contains("unsupported"));
}

#[test]
fn context_engine_strict_mode_fails_when_drop_fails() {
    let tmp = tempdir().expect("tempdir");
    let moon_home = tmp.path().join("moon");
    fs::create_dir_all(&moon_home).expect("mkdir moon");
    let moon_home = fs::canonicalize(&moon_home).expect("canonicalize moon");
    write_moon_env(&moon_home);
    write_moon_config(&moon_home, "strict", None);

    let source_a = moon_home.join("session-a.jsonl");
    let source_b = moon_home.join("session-b.jsonl");
    write_source(&source_a, "session a");
    write_source(&source_b, "session b");

    let qmd = tmp.path().join("qmd");
    write_executable(
        &qmd,
        r#"#!/usr/bin/env bash
set -euo pipefail

	if [[ "${1:-}" == "collection" && "${2:-}" == "add" ]]; then
	  exit 0
	fi
	if [[ "${1:-}" == "collection" && "${2:-}" == "show" ]]; then
	  exit 0
	fi
	if [[ "${1:-}" == "collection" && "${2:-}" == "remove" ]]; then
	  echo "drop denied" >&2
	  exit 2
	fi
	if [[ "${1:-}" == "collection" && "${2:-}" == "--help" ]]; then
	  echo "Commands: add remove show"
	  exit 0
	fi

echo "unknown command" >&2
exit 1
"#,
    );

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(&moon_home)
        .env("MOON_HOME", &moon_home)
        .env("QMD_BIN", &qmd)
        .arg("context-engine")
        .args(["--source", &source_a.display().to_string()])
        .args(["--session-id", "strict-drop-a"])
        .args(["--used-tokens", "1000"])
        .args(["--max-tokens", "200000"])
        .assert()
        .success();

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(&moon_home)
        .env("MOON_HOME", &moon_home)
        .env("QMD_BIN", &qmd)
        .arg("context-engine")
        .args(["--source", &source_b.display().to_string()])
        .args(["--session-id", "strict-drop-b"])
        .args(["--used-tokens", "1000"])
        .args(["--max-tokens", "200000"])
        .assert()
        .failure()
        .stderr(contains("hot collection lifecycle failed in strict mode"))
        .stderr(contains("drop"));
}

#[test]
fn context_engine_supports_fallback_lifecycle_command_shapes() {
    let tmp = tempdir().expect("tempdir");
    let moon_home = tmp.path().join("moon");
    fs::create_dir_all(&moon_home).expect("mkdir moon");
    let moon_home = fs::canonicalize(&moon_home).expect("canonicalize moon");
    write_moon_env(&moon_home);
    write_moon_config(&moon_home, "strict", Some("fallback"));

    let source_a = moon_home.join("session-a.jsonl");
    let source_b = moon_home.join("session-b.jsonl");
    write_source(&source_a, "session a");
    write_source(&source_b, "session b");

    let qmd = tmp.path().join("qmd");
    let qmd_log = tmp.path().join("qmd.log");
    write_executable(
        &qmd,
        &format!(
            r#"#!/usr/bin/env bash
set -euo pipefail
echo "$*" >> "{}"

	if [[ "${{1:-}}" == "collection" ]]; then
	  echo "unknown command: collection" >&2
	  exit 1
	fi

	if [[ "${{1:-}}" == "create" && "${{2:-}}" == "--help" ]]; then
	  echo "Usage: qmd create <name>"
	  exit 0
	fi
	if [[ "${{1:-}}" == "switch" && "${{2:-}}" == "--help" ]]; then
	  echo "Usage: qmd switch <name>"
	  exit 0
	fi
	if [[ "${{1:-}}" == "drop" && "${{2:-}}" == "--help" ]]; then
	  echo "Usage: qmd drop <name>"
	  exit 0
	fi
	if [[ "${{1:-}}" == "create" || "${{1:-}}" == "switch" || "${{1:-}}" == "drop" ]]; then
	  exit 0
	fi

echo "unknown command" >&2
exit 1
"#,
            qmd_log.display()
        ),
    );

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(&moon_home)
        .env("MOON_HOME", &moon_home)
        .env("QMD_BIN", &qmd)
        .arg("context-engine")
        .args(["--source", &source_a.display().to_string()])
        .args(["--session-id", "fb-a"])
        .args(["--used-tokens", "1000"])
        .args(["--max-tokens", "200000"])
        .assert()
        .success();

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(&moon_home)
        .env("MOON_HOME", &moon_home)
        .env("QMD_BIN", &qmd)
        .arg("context-engine")
        .args(["--source", &source_b.display().to_string()])
        .args(["--session-id", "fb-b"])
        .args(["--used-tokens", "1000"])
        .args(["--max-tokens", "200000"])
        .assert()
        .success();

    let qmd_log = fs::read_to_string(&qmd_log).expect("read qmd log");
    assert!(qmd_log.contains("create history_hot_fb-a"));
    assert!(qmd_log.contains("drop history_hot_fb-a"));
    assert!(!qmd_log.contains("collection add"));
    assert!(!qmd_log.contains("collection remove"));
}

#[test]
fn context_engine_degrade_mode_skips_unsupported_lifecycle_commands_after_probe() {
    let tmp = tempdir().expect("tempdir");
    let moon_home = tmp.path().join("moon");
    fs::create_dir_all(&moon_home).expect("mkdir moon");
    let moon_home = fs::canonicalize(&moon_home).expect("canonicalize moon");
    write_moon_env(&moon_home);
    write_moon_config(&moon_home, "degrade", None);

    let source = moon_home.join("session-a.jsonl");
    write_source(&source, "session a");

    let qmd = tmp.path().join("qmd");
    let qmd_log = tmp.path().join("qmd.log");
    write_executable(
        &qmd,
        &format!(
            r#"#!/usr/bin/env bash
set -euo pipefail
echo "$*" >> "{}"

if [[ "${{1:-}}" == "collection" && "${{2:-}}" == "--help" ]]; then
  echo "Commands: add list rename"
  exit 0
fi

echo "unknown command" >&2
exit 1
"#,
            qmd_log.display()
        ),
    );

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(&moon_home)
        .env("MOON_HOME", &moon_home)
        .env("QMD_BIN", &qmd)
        .arg("context-engine")
        .args(["--source", &source.display().to_string()])
        .args(["--session-id", "degrade-a"])
        .args(["--used-tokens", "1000"])
        .args(["--max-tokens", "200000"])
        .assert()
        .success();

    let qmd_log = fs::read_to_string(&qmd_log).expect("read qmd log");
    assert!(qmd_log.contains("collection --help"));
    assert!(!qmd_log.contains("collection add"));
    assert!(!qmd_log.contains("collection remove"));
}
