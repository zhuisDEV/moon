#![cfg(not(windows))]
use predicates::str::contains;
use std::fs;
use std::path::Path;
use tempfile::tempdir;

fn write_fake_qmd_bounded(bin_path: &Path, log_path: &Path) {
    let script = format!(
        r#"#!/usr/bin/env bash
set -euo pipefail
echo "$*" >> "{}"

if [[ "${{1:-}}" == "embed" && "${{2:-}}" == "--help" ]]; then
  echo "Usage: qmd embed <collection> --max-docs <n>"
  exit 0
fi

if [[ "${{1:-}}" == "embed" ]]; then
  exit 0
fi

exit 0
"#,
        log_path.display()
    );
    fs::write(bin_path, script).expect("write fake qmd");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(bin_path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(bin_path, perms).expect("chmod");
    }
}

fn write_fake_qmd_missing_capability(bin_path: &Path) {
    let script = r#"#!/usr/bin/env bash
set -euo pipefail

if [[ "${1:-}" == "embed" && "${2:-}" == "--help" ]]; then
  echo "unknown command: embed" >&2
  exit 1
fi

exit 0
"#;
    fs::write(bin_path, script).expect("write fake qmd");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(bin_path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(bin_path, perms).expect("chmod");
    }
}

fn write_fake_qmd_unbounded_only(bin_path: &Path, log_path: &Path) {
    let script = format!(
        r#"#!/usr/bin/env bash
set -euo pipefail
echo "$*" >> "{}"

if [[ "${{1:-}}" == "embed" && "${{2:-}}" == "--help" ]]; then
  echo "Usage: qmd embed <collection>"
  exit 0
fi

if [[ "${{1:-}}" == "embed" ]]; then
  exit 0
fi

exit 0
"#,
        log_path.display()
    );
    fs::write(bin_path, script).expect("write fake qmd");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(bin_path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(bin_path, perms).expect("chmod");
    }
}

fn write_moon_env(moon_home: &Path) {
    fs::create_dir_all(moon_home).expect("mkdir moon root");
    fs::write(moon_home.join(".env"), "\n").expect("write moon .env");
}

#[test]
fn moon_embed_runs_bounded_and_updates_state() {
    let tmp = tempdir().expect("tempdir");
    let moon_home = tmp.path().join("moon");
    let mds_dir = moon_home.join("mds");
    let hot_dir = mds_dir.join("history_hot");
    fs::create_dir_all(&mds_dir).expect("mkdir mds");
    fs::create_dir_all(&hot_dir).expect("mkdir hot");
    fs::create_dir_all(moon_home.join("memory")).expect("mkdir memory");
    fs::create_dir_all(moon_home.join("logs")).expect("mkdir logs");
    write_moon_env(&moon_home);

    fs::write(hot_dir.join("a.md"), "a").expect("write a");
    fs::write(hot_dir.join("b.md"), "b").expect("write b");

    let qmd = tmp.path().join("qmd");
    let qmd_log = tmp.path().join("qmd.log");
    write_fake_qmd_bounded(&qmd, &qmd_log);

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("QMD_BIN", &qmd)
        .arg("--json")
        .arg("embed")
        .args(["--name", "history_hot"])
        .args(["--max-docs", "1"])
        .assert()
        .success()
        .stdout(contains("embed.selected_docs=1"))
        .stdout(contains("embed.pending_before=2"))
        .stdout(contains("embed.pending_after=1"));

    let log = fs::read_to_string(&qmd_log).expect("read qmd log");
    assert!(log.contains("embed --help"));
    assert!(log.contains("embed history_hot --max-docs 1"));
}

#[test]
fn moon_embed_manual_fails_when_capability_missing() {
    let tmp = tempdir().expect("tempdir");
    let moon_home = tmp.path().join("moon");
    let mds_dir = moon_home.join("mds");
    let hot_dir = mds_dir.join("history_hot");
    fs::create_dir_all(&mds_dir).expect("mkdir mds");
    fs::create_dir_all(&hot_dir).expect("mkdir hot");
    fs::create_dir_all(moon_home.join("memory")).expect("mkdir memory");
    fs::create_dir_all(moon_home.join("logs")).expect("mkdir logs");
    write_moon_env(&moon_home);
    fs::write(hot_dir.join("x.md"), "x").expect("write x");

    let qmd = tmp.path().join("qmd");
    write_fake_qmd_missing_capability(&qmd);

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("QMD_BIN", &qmd)
        .arg("embed")
        .args(["--name", "history_hot"])
        .assert()
        .failure()
        .stdout(contains("embed capability missing"));
}

#[test]
fn moon_embed_watcher_trigger_degrades_on_missing_capability() {
    let tmp = tempdir().expect("tempdir");
    let moon_home = tmp.path().join("moon");
    let mds_dir = moon_home.join("mds");
    let hot_dir = mds_dir.join("history_hot");
    fs::create_dir_all(&mds_dir).expect("mkdir mds");
    fs::create_dir_all(&hot_dir).expect("mkdir hot");
    fs::create_dir_all(moon_home.join("memory")).expect("mkdir memory");
    fs::create_dir_all(moon_home.join("logs")).expect("mkdir logs");
    write_moon_env(&moon_home);
    fs::write(hot_dir.join("x.md"), "x").expect("write x");

    let qmd = tmp.path().join("qmd");
    write_fake_qmd_missing_capability(&qmd);

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("QMD_BIN", &qmd)
        .arg("embed")
        .args(["--name", "history_hot"])
        .arg("--watcher-trigger")
        .assert()
        .success()
        .stdout(contains("embed.skip_reason=capability-missing"));
}

#[test]
fn moon_embed_manual_uses_global_embed_when_only_unbounded_capability_exists() {
    let tmp = tempdir().expect("tempdir");
    let moon_home = tmp.path().join("moon");
    let mds_dir = moon_home.join("mds");
    let hot_dir = mds_dir.join("history_hot");
    fs::create_dir_all(&mds_dir).expect("mkdir mds");
    fs::create_dir_all(&hot_dir).expect("mkdir hot");
    fs::create_dir_all(moon_home.join("memory")).expect("mkdir memory");
    fs::create_dir_all(moon_home.join("logs")).expect("mkdir logs");
    write_moon_env(&moon_home);
    fs::write(hot_dir.join("a.md"), "a").expect("write a");
    fs::write(hot_dir.join("b.md"), "b").expect("write b");

    let qmd = tmp.path().join("qmd");
    let qmd_log = tmp.path().join("qmd.log");
    write_fake_qmd_unbounded_only(&qmd, &qmd_log);

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("QMD_BIN", &qmd)
        .arg("--json")
        .arg("embed")
        .args(["--name", "history_hot"])
        .assert()
        .success()
        .stdout(contains("embed.capability=unbounded-only"))
        .stdout(contains("embed.selected_docs=2"))
        .stdout(contains("embed.pending_before=2"))
        .stdout(contains("embed.pending_after=0"));

    let log = fs::read_to_string(&qmd_log).expect("read qmd log");
    assert!(log.contains("embed --help"));
    assert!(log.contains("embed --max-docs-per-batch 2"));
    assert!(!log.contains("embed history_hot"));
}

#[test]
fn moon_embed_manual_ignores_watcher_cooldown_and_keeps_watcher_clock() {
    let tmp = tempdir().expect("tempdir");
    let moon_home = tmp.path().join("moon");
    let mds_dir = moon_home.join("mds");
    let hot_dir = mds_dir.join("history_hot");
    fs::create_dir_all(&mds_dir).expect("mkdir mds");
    fs::create_dir_all(&hot_dir).expect("mkdir hot");
    fs::create_dir_all(moon_home.join("memory")).expect("mkdir memory");
    fs::create_dir_all(moon_home.join("logs")).expect("mkdir logs");
    write_moon_env(&moon_home);

    fs::write(hot_dir.join("a.md"), "a").expect("write a");
    fs::write(hot_dir.join("b.md"), "b").expect("write b");

    let qmd = tmp.path().join("qmd");
    let qmd_log = tmp.path().join("qmd.log");
    write_fake_qmd_bounded(&qmd, &qmd_log);

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("QMD_BIN", &qmd)
        .env("MOON_EMBED_COOLDOWN_SECS", "3600")
        .arg("--json")
        .arg("embed")
        .args(["--name", "history_hot"])
        .args(["--max-docs", "1"])
        .arg("--watcher-trigger")
        .assert()
        .success()
        .stdout(contains("embed.selected_docs=1"))
        .stdout(contains("embed.pending_before=2"))
        .stdout(contains("embed.pending_after=1"));

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("QMD_BIN", &qmd)
        .env("MOON_EMBED_COOLDOWN_SECS", "3600")
        .arg("--json")
        .arg("embed")
        .args(["--name", "history_hot"])
        .args(["--max-docs", "1"])
        .arg("--watcher-trigger")
        .assert()
        .success()
        .stdout(contains("embed.skip_reason=cooldown"));

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("QMD_BIN", &qmd)
        .env("MOON_EMBED_COOLDOWN_SECS", "3600")
        .arg("--json")
        .arg("embed")
        .args(["--name", "history_hot"])
        .args(["--max-docs", "1"])
        .assert()
        .success()
        .stdout(contains("embed.selected_docs=1"))
        .stdout(contains("embed.pending_before=1"))
        .stdout(contains("embed.pending_after=0"));

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("QMD_BIN", &qmd)
        .env("MOON_EMBED_COOLDOWN_SECS", "3600")
        .arg("--json")
        .arg("embed")
        .args(["--name", "history_hot"])
        .args(["--max-docs", "1"])
        .arg("--watcher-trigger")
        .assert()
        .success()
        .stdout(contains("embed.skip_reason=cooldown"));
}
