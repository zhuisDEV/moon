#![cfg(unix)]

use assert_cmd::Command;
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use tempfile::TempDir;

fn fake_codex(temp: &TempDir) -> std::path::PathBuf {
    let path = temp.path().join("codex");
    fs::write(
        &path,
        r#"#!/bin/sh
set -eu
if [ "${1:-}" = "login" ] && [ "${2:-}" = "status" ]; then
  echo "Logged in using ChatGPT"
  exit 0
fi
if [ "${1:-}" = "exec" ]; then
  output=""
  previous=""
  for argument in "$@"; do
    if [ "$previous" = "--output-last-message" ]; then
      output="$argument"
    fi
    previous="$argument"
  done
  if [ -n "${CODEX_HOME:-}" ]; then
    echo "OAuth expired" >&2
    exit 1
  fi
  prompt="$(cat)"
  [ "$prompt" = "Return READY." ]
  [ "${argument:-}" = "-" ]
  printf "READY\n" > "$output"
  exit 0
fi
echo "unexpected fake codex command" >&2
exit 2
"#,
    )
    .expect("write fake codex");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
        .expect("make fake codex executable");
    path
}

#[test]
fn auth_status_preserves_priority_without_creating_storage() {
    let temp = TempDir::new().expect("temp");
    let moon_home = temp.path().join("moon-home");
    let codex = fake_codex(&temp);
    let output = Command::new(assert_cmd::cargo::cargo_bin!("moon"))
        .args([
            "--home",
            moon_home.to_str().expect("home"),
            "--json",
            "auth",
            "status",
            "--openclaw-available",
        ])
        .env("MOON_CODEX_PATH", codex)
        .env_remove("CODEX_HOME")
        .output()
        .expect("run auth status");
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).expect("status JSON");
    assert_eq!(value["selected"], "open-claw");
    assert_eq!(value["checks"][1]["level"], "moon");
    assert_eq!(value["checks"][2]["level"], "codex");
    assert!(!moon_home.exists(), "status must remain read-only");
}

#[test]
fn auth_exec_falls_back_only_after_private_codex_auth_expires() {
    let temp = TempDir::new().expect("temp");
    let moon_home = temp.path().join("moon-home");
    let codex = fake_codex(&temp);
    let output = Command::new(assert_cmd::cargo::cargo_bin!("moon"))
        .args([
            "--home",
            moon_home.to_str().expect("home"),
            "--json",
            "auth",
            "exec",
            "--model",
            "gpt-5.6-sol",
        ])
        .env("MOON_CODEX_PATH", codex)
        .env_remove("CODEX_HOME")
        .write_stdin("Return READY.")
        .output()
        .expect("run auth exec");
    assert!(output.status.success(), "{:?}", output);
    let value: Value = serde_json::from_slice(&output.stdout).expect("outcome JSON");
    assert_eq!(value["auth_level"], "codex");
    assert_eq!(value["model"], "gpt-5.6-sol");
    assert_eq!(value["reasoning"], "high");
    assert_eq!(value["output"], "READY");
    assert!(
        !moon_home.join("state/moon.sqlite").exists(),
        "auth must not open memory storage"
    );
}
