#![cfg(unix)]

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use std::fs;
use std::os::unix::fs::symlink;

#[test]
fn plain_version_is_one_offline_line_without_a_runtime() {
    let temp = tempfile::tempdir().expect("tempdir");
    let moon_home = temp.path().join("missing-moon-home");

    Command::new(assert_cmd::cargo::cargo_bin!("moon"))
        .arg("--version")
        .env("MOON_HOME", &moon_home)
        .assert()
        .success()
        .stdout(predicate::eq(format!(
            "moon {}\n",
            env!("CARGO_PKG_VERSION")
        )))
        .stderr(predicate::str::is_empty());

    assert!(!moon_home.exists(), "version must not create a runtime");
}

#[test]
fn json_version_reports_build_and_executable_provenance_offline() {
    let temp = tempfile::tempdir().expect("tempdir");
    let moon_home = temp.path().join("missing-moon-home");
    let output = Command::new(assert_cmd::cargo::cargo_bin!("moon"))
        .args(["--json", "--version"])
        .env("MOON_HOME", &moon_home)
        .output()
        .expect("run version");

    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty());
    let value: Value = serde_json::from_slice(&output.stdout).expect("version JSON");
    assert_eq!(value["ok"], true);
    assert_eq!(value["name"], "moon");
    assert_eq!(value["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(value["bundle_format"], 1);
    assert_eq!(value["canonical"], false);
    assert_eq!(
        value["canonical_executable"],
        moon_home.join("bin/moon").to_string_lossy().as_ref()
    );
    assert!(
        value["git_commit"] == "unknown"
            || value["git_commit"]
                .as_str()
                .is_some_and(|commit| commit.len() >= 40)
    );
    assert!(value["git_dirty"].is_boolean() || value["git_dirty"].is_null());
    assert!(
        value["build_target"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
    assert!(
        value["build_profile"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
    assert!(
        value["executable"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
    assert!(!moon_home.exists(), "version must not create a runtime");
}

#[test]
fn json_version_recognizes_the_canonical_executable_by_file_identity() {
    let temp = tempfile::tempdir().expect("tempdir");
    let moon_home = temp.path().join("moon-home");
    let bin_dir = moon_home.join("bin");
    fs::create_dir_all(&bin_dir).expect("create bin fixture");
    let canonical = bin_dir.join("moon");
    symlink(assert_cmd::cargo::cargo_bin!("moon"), &canonical).expect("link canonical binary");

    let output = Command::new(&canonical)
        .args(["--version", "--json"])
        .env("MOON_HOME", &moon_home)
        .output()
        .expect("run canonical version");

    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).expect("version JSON");
    assert_eq!(value["canonical"], true);
    assert_eq!(
        value["canonical_executable"],
        canonical.to_string_lossy().as_ref()
    );
}
