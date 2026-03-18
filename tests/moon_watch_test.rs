#![cfg(not(windows))]
use chrono::{Duration as ChronoDuration, TimeZone, Utc};
use predicates::str::contains;
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use tempfile::tempdir;

fn write_fake_qmd(bin_path: &Path) {
    let script = r#"#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == "collection" && "${2:-}" == "--help" ]]; then
  echo "Commands: add remove show"
  exit 0
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
    fs::write(bin_path, script).expect("write fake qmd bounded");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(bin_path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(bin_path, perms).expect("chmod");
    }
}

fn write_fake_qmd_missing_embed_capability(bin_path: &Path) {
    let script = r#"#!/usr/bin/env bash
set -euo pipefail

if [[ "${1:-}" == "embed" && "${2:-}" == "--help" ]]; then
  echo "unknown command: embed" >&2
  exit 1
fi

	if [[ "${1:-}" == "collection" ]]; then
	  if [[ "${2:-}" == "--help" ]]; then
	    echo "Commands: add remove show"
	  fi
	  exit 0
	fi

exit 0
"#;
    fs::write(bin_path, script).expect("write fake qmd missing embed capability");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(bin_path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(bin_path, perms).expect("chmod");
    }
}

fn write_fake_qmd_hot_lifecycle_unsupported(bin_path: &Path) {
    let script = r#"#!/usr/bin/env bash
set -euo pipefail

if [[ "${1:-}" == "embed" && "${2:-}" == "--help" ]]; then
  echo "Usage: qmd embed <collection> --max-docs <n>"
  exit 0
fi

if [[ "${1:-}" == "embed" ]]; then
  exit 0
fi

if [[ "${1:-}" == "collection" || "${1:-}" == "collections" ]]; then
  echo "unknown command: collection" >&2
  exit 1
fi

if [[ "${1:-}" == "create" || "${1:-}" == "switch" || "${1:-}" == "use" || "${1:-}" == "drop" || "${1:-}" == "remove" ]]; then
  echo "unknown command" >&2
  exit 1
fi

exit 0
"#;
    fs::write(bin_path, script).expect("write fake qmd hot lifecycle unsupported");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(bin_path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(bin_path, perms).expect("chmod");
    }
}

fn write_fake_openclaw(bin_path: &Path) {
    let script = r#"#!/usr/bin/env bash
set -euo pipefail
exit 0
"#;
    fs::write(bin_path, script).expect("write fake openclaw");
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

fn write_moon_config(moon_home: &Path, lifecycle_mode: &str, lifecycle_command_mode: Option<&str>) {
    let mut config = format!("[hot_collection]\nlifecycle_mode = \"{lifecycle_mode}\"\n");
    if let Some(command_mode) = lifecycle_command_mode {
        config.push_str(&format!("lifecycle_command_mode = \"{command_mode}\"\n"));
    }
    fs::write(moon_home.join("moon.toml"), config).expect("write moon.toml");
}

fn read_distilled_paths(state_file: &Path) -> Vec<String> {
    let raw = fs::read_to_string(state_file).expect("read state");
    let parsed: Value = serde_json::from_str(&raw).expect("parse state");
    let map = parsed
        .get("distilled_archives")
        .and_then(Value::as_object)
        .expect("distilled_archives map");
    map.keys().cloned().collect()
}

#[test]
fn moon_watch_once_uses_moon_state_file_override() {
    let tmp = tempdir().expect("tempdir");
    let moon_home = tmp.path().join("moon");
    let sessions_dir = tmp.path().join("sessions");
    fs::create_dir_all(moon_home.join("memory")).expect("mkdir memory");
    fs::create_dir_all(moon_home.join("logs")).expect("mkdir logs");
    fs::create_dir_all(&sessions_dir).expect("mkdir sessions");
    write_moon_env(&moon_home);

    let qmd = tmp.path().join("qmd");
    write_fake_qmd(&qmd);
    let openclaw = tmp.path().join("openclaw");
    write_fake_openclaw(&openclaw);

    let custom_state_file = tmp.path().join("custom-state").join("moon_state.json");

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("MOON_STATE_FILE", &custom_state_file)
        .env("OPENCLAW_SESSIONS_DIR", &sessions_dir)
        .env("QMD_BIN", &qmd)
        .env("OPENCLAW_BIN", &openclaw)
        .arg("watch")
        .arg("--once")
        .assert()
        .success()
        .stdout(contains(format!(
            "state_file={}",
            custom_state_file.display()
        )));

    assert!(custom_state_file.exists());
    assert!(!moon_home.join("state/moon_state.json").exists());
}

#[test]
fn moon_watch_once_dry_run_skips_state_write() {
    let tmp = tempdir().expect("tempdir");
    let moon_home = tmp.path().join("moon");
    let sessions_dir = tmp.path().join("sessions");
    fs::create_dir_all(moon_home.join("memory")).expect("mkdir memory");
    fs::create_dir_all(moon_home.join("logs")).expect("mkdir logs");
    fs::create_dir_all(&sessions_dir).expect("mkdir sessions");
    write_moon_env(&moon_home);

    let qmd = tmp.path().join("qmd");
    write_fake_qmd(&qmd);
    let openclaw = tmp.path().join("openclaw");
    write_fake_openclaw(&openclaw);

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("OPENCLAW_SESSIONS_DIR", &sessions_dir)
        .env("QMD_BIN", &qmd)
        .env("OPENCLAW_BIN", &openclaw)
        .arg("watch")
        .arg("--once")
        .arg("--dry-run")
        .assert()
        .success()
        .stdout(contains("dry_run=true"));

    assert!(!moon_home.join("state/moon_state.json").exists());
}

#[test]
fn moon_watch_once_distills_pending_mlib_docs() {
    let tmp = tempdir().expect("tempdir");
    let moon_home = tmp.path().join("moon");
    let sessions_dir = tmp.path().join("sessions");
    let mlib_dir = moon_home.join("mlib");
    fs::create_dir_all(&mlib_dir).expect("mkdir mlib");
    fs::create_dir_all(moon_home.join("memory")).expect("mkdir memory");
    fs::create_dir_all(moon_home.join("logs")).expect("mkdir logs");
    fs::create_dir_all(&sessions_dir).expect("mkdir sessions");
    write_moon_env(&moon_home);

    fs::write(
        mlib_dir.join("fresh.md"),
        "# MOON Archive Markdown\n\nDecision: simplify the primary flow.\n",
    )
    .expect("write mlib");

    let qmd = tmp.path().join("qmd");
    write_fake_qmd(&qmd);
    let openclaw = tmp.path().join("openclaw");
    write_fake_openclaw(&openclaw);

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("OPENCLAW_SESSIONS_DIR", &sessions_dir)
        .env("QMD_BIN", &qmd)
        .env("OPENCLAW_BIN", &openclaw)
        .env("MOON_DISTILL_PROVIDER", "local")
        .env("MOON_DISTILL_MAX_PER_CYCLE", "1")
        .arg("watch")
        .arg("--once")
        .assert()
        .success()
        .stdout(contains("pending_mlib_docs=1"))
        .stdout(contains("distill.runs=1"));

    let state_file = moon_home.join("state/moon_state.json");
    let distilled = read_distilled_paths(&state_file);
    assert_eq!(distilled.len(), 1);
    assert!(distilled[0].contains("/mlib/fresh.md"));
}

#[test]
fn moon_watch_once_projects_and_embeds_pending_library_maintenance() {
    let tmp = tempdir().expect("tempdir");
    let moon_home = tmp.path().join("moon");
    let sessions_dir = tmp.path().join("sessions");
    let raw_dir = moon_home.join("raw");
    fs::create_dir_all(&raw_dir).expect("mkdir raw");
    fs::create_dir_all(moon_home.join("mlib")).expect("mkdir mlib");
    fs::create_dir_all(moon_home.join("memory")).expect("mkdir memory");
    fs::create_dir_all(moon_home.join("logs")).expect("mkdir logs");
    fs::create_dir_all(&sessions_dir).expect("mkdir sessions");
    write_moon_env(&moon_home);

    let raw_path = raw_dir.join("session-cleanse.jsonl");
    fs::write(
        &raw_path,
        "{\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"Queue projection and embed after compaction.\"}]}}\n",
    )
    .expect("write raw session");

    let qmd = tmp.path().join("qmd");
    let qmd_log = tmp.path().join("qmd.log");
    write_fake_qmd_bounded(&qmd, &qmd_log);
    let openclaw = tmp.path().join("openclaw");
    write_fake_openclaw(&openclaw);

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("OPENCLAW_SESSIONS_DIR", &sessions_dir)
        .env("QMD_BIN", &qmd)
        .env("OPENCLAW_BIN", &openclaw)
        .env("MOON_DISTILL_PROVIDER", "local")
        .arg("watch")
        .arg("--once")
        .assert()
        .success()
        .stdout(contains("pending_raw_sessions=1"))
        .stdout(contains("project.runs=1"))
        .stdout(contains("pending_embed_collections=1"))
        .stdout(contains("embed.runs=1"));

    assert!(moon_home.join("mlib/session-cleanse.md").exists());

    let qmd_log_raw = fs::read_to_string(&qmd_log).expect("read qmd log");
    assert!(qmd_log_raw.contains("embed --help"));
    assert!(qmd_log_raw.contains("embed history_lib --max-docs"));

    let state_file = moon_home.join("state/moon_state.json");

    let next_state: Value =
        serde_json::from_str(&fs::read_to_string(&state_file).expect("read state"))
            .expect("parse state");
    let bytes = fs::metadata(&raw_path).expect("stat raw").len();
    let lines = 1_u64;
    assert_eq!(
        next_state
            .get("raw_session_cursors")
            .and_then(|v| v.get("session-cleanse"))
            .and_then(|v| v.get("bytes"))
            .and_then(Value::as_u64),
        Some(bytes)
    );
    assert_eq!(
        next_state
            .get("raw_session_cursors")
            .and_then(|v| v.get("session-cleanse"))
            .and_then(|v| v.get("lines"))
            .and_then(Value::as_u64),
        Some(lines)
    );
    assert_eq!(
        next_state
            .get("pending_embed_collections")
            .and_then(Value::as_object)
            .map(|map| map.len()),
        Some(0)
    );
}

#[test]
fn moon_watch_once_runs_midnight_syns_from_yesterday_and_memory() {
    let tmp = tempdir().expect("tempdir");
    let moon_home = tmp.path().join("moon");
    let sessions_dir = tmp.path().join("sessions");
    fs::create_dir_all(moon_home.join("memory")).expect("mkdir memory");
    fs::create_dir_all(moon_home.join("logs")).expect("mkdir logs");
    fs::create_dir_all(moon_home.join("state")).expect("mkdir state");
    fs::create_dir_all(&sessions_dir).expect("mkdir sessions");
    write_moon_env(&moon_home);
    write_moon_config(&moon_home, "strict", None);

    let now_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("epoch")
        .as_secs();
    let now_utc = Utc
        .timestamp_opt(now_epoch as i64, 0)
        .single()
        .expect("utc timestamp");
    let yesterday = (now_utc.date_naive() - ChronoDuration::days(1))
        .format("%Y-%m-%d")
        .to_string();
    let yesterday_file = moon_home.join("memory").join(format!("{yesterday}.md"));
    let memory_file = moon_home.join("MEMORY.md");
    fs::write(
        &yesterday_file,
        "# Daily Memory\n<!-- moon_memory_format: conversation_v1 -->\n\n## Session y1\n**User:** Keep workflow simple.\n**Assistant:** Use one path.\n",
    )
    .expect("write yesterday memory");
    fs::write(
        &memory_file,
        "# MEMORY\n\n## Durable\n- Keep summaries concise.\n",
    )
    .expect("write memory file");

    let midnight_state = "{\n  \"schema_version\": 3,\n  \"last_heartbeat_epoch_secs\": 0,\n  \"last_archive_trigger_epoch_secs\": null,\n  \"last_compaction_trigger_epoch_secs\": null,\n  \"last_distill_trigger_epoch_secs\": null,\n  \"last_syns_trigger_epoch_secs\": null,\n  \"last_embed_trigger_epoch_secs\": null,\n  \"last_session_id\": null,\n  \"last_usage_ratio\": null,\n  \"last_provider\": null,\n  \"distilled_archives\": {},\n  \"embedded_projections\": {},\n  \"inbound_seen_files\": {}\n}\n".to_string();
    fs::write(moon_home.join("state/moon_state.json"), midnight_state).expect("write state");

    let fake_midnight_epoch = now_utc
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .expect("midnight")
        .and_utc()
        .timestamp() as u64;

    let qmd = tmp.path().join("qmd");
    write_fake_qmd(&qmd);
    let openclaw = tmp.path().join("openclaw");
    write_fake_openclaw(&openclaw);

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("OPENCLAW_SESSIONS_DIR", &sessions_dir)
        .env("QMD_BIN", &qmd)
        .env("OPENCLAW_BIN", &openclaw)
        .env("MOON_RESIDENTIAL_TIMEZONE", "UTC")
        .env("MOON_WISDOM_PROVIDER", "local")
        .env(
            "MOON_WATCH_FAKE_NOW_EPOCH_SECS",
            fake_midnight_epoch.to_string(),
        )
        .arg("watch")
        .arg("--once")
        .assert()
        .success();

    let state_raw =
        fs::read_to_string(moon_home.join("state/moon_state.json")).expect("read state");
    assert!(state_raw.contains("\"last_syns_trigger_epoch_secs\":"));
}

#[test]
fn moon_watch_once_strict_mode_fails_on_degraded_hot_embed_result() {
    let tmp = tempdir().expect("tempdir");
    let moon_home = tmp.path().join("moon");
    let sessions_dir = tmp.path().join("sessions");
    fs::create_dir_all(moon_home.join("mds")).expect("mkdir mds");
    fs::create_dir_all(moon_home.join("mlib")).expect("mkdir mlib");
    fs::create_dir_all(moon_home.join("memory")).expect("mkdir memory");
    fs::create_dir_all(moon_home.join("logs")).expect("mkdir logs");
    fs::create_dir_all(moon_home.join("state")).expect("mkdir state");
    fs::create_dir_all(&sessions_dir).expect("mkdir sessions");
    write_moon_env(&moon_home);
    write_moon_config(&moon_home, "strict", None);

    fs::create_dir_all(moon_home.join("mds/history_hot_session-strict")).expect("mkdir hot dir");
    fs::write(
        moon_home.join("mds/history_hot_session-strict/session.md"),
        "# hot projection for strict watcher\n",
    )
    .expect("write hot projection");
    fs::write(
        moon_home.join("state/moon_state.json"),
        r#"{
  "schema_version": 6,
  "last_heartbeat_epoch_secs": 0,
  "distilled_archives": {},
  "embedded_projections": {},
  "pending_embed_collections": {
    "history_hot_session-strict": 1
  },
  "inbound_seen_files": {}
}
"#,
    )
    .expect("write state");

    let qmd = tmp.path().join("qmd");
    write_fake_qmd_missing_embed_capability(&qmd);
    let openclaw = tmp.path().join("openclaw");
    write_fake_openclaw(&openclaw);

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("OPENCLAW_SESSIONS_DIR", &sessions_dir)
        .env("QMD_BIN", &qmd)
        .env("OPENCLAW_BIN", &openclaw)
        .arg("watch")
        .arg("--once")
        .assert()
        .failure()
        .stderr(contains(
            "watcher strict mode rejects degraded embed result",
        ));
}

#[test]
fn moon_watch_once_strict_mode_allows_degraded_library_embed_result() {
    let tmp = tempdir().expect("tempdir");
    let moon_home = tmp.path().join("moon");
    let sessions_dir = tmp.path().join("sessions");
    let raw_dir = moon_home.join("raw");
    fs::create_dir_all(&raw_dir).expect("mkdir raw");
    fs::create_dir_all(moon_home.join("mlib")).expect("mkdir mlib");
    fs::create_dir_all(moon_home.join("memory")).expect("mkdir memory");
    fs::create_dir_all(moon_home.join("logs")).expect("mkdir logs");
    fs::create_dir_all(&sessions_dir).expect("mkdir sessions");
    write_moon_env(&moon_home);
    write_moon_config(&moon_home, "strict", None);

    fs::write(
        raw_dir.join("session-lib.jsonl"),
        "{\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"strict watcher library test\"}]}}\n",
    )
    .expect("write raw session");

    let qmd = tmp.path().join("qmd");
    write_fake_qmd_missing_embed_capability(&qmd);
    let openclaw = tmp.path().join("openclaw");
    write_fake_openclaw(&openclaw);

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("OPENCLAW_SESSIONS_DIR", &sessions_dir)
        .env("QMD_BIN", &qmd)
        .env("OPENCLAW_BIN", &openclaw)
        .env("MOON_DISTILL_PROVIDER", "local")
        .arg("watch")
        .arg("--once")
        .assert()
        .success()
        .stdout(contains("collection=history_lib"))
        .stdout(contains("degraded=true"));
}

#[test]
fn moon_watch_once_strict_mode_fails_when_hot_collection_lifecycle_unsupported() {
    let tmp = tempdir().expect("tempdir");
    let moon_home = tmp.path().join("moon");
    let sessions_dir = tmp.path().join("sessions");
    fs::create_dir_all(moon_home.join("mds")).expect("mkdir mds");
    fs::create_dir_all(moon_home.join("mlib")).expect("mkdir mlib");
    fs::create_dir_all(moon_home.join("memory")).expect("mkdir memory");
    fs::create_dir_all(moon_home.join("logs")).expect("mkdir logs");
    fs::create_dir_all(moon_home.join("state")).expect("mkdir state");
    fs::create_dir_all(&sessions_dir).expect("mkdir sessions");
    write_moon_env(&moon_home);
    write_moon_config(&moon_home, "strict", None);

    fs::create_dir_all(moon_home.join("mds/history_hot_session-hot")).expect("mkdir hot dir");
    fs::write(
        moon_home.join("mds/history_hot_session-hot/session.md"),
        "# hot projection\n",
    )
    .expect("write mds doc");
    fs::write(
        moon_home.join("state/moon_state.json"),
        r#"{
  "schema_version": 6,
  "last_heartbeat_epoch_secs": 0,
  "distilled_archives": {},
  "embedded_projections": {},
  "pending_embed_collections": {
    "history_hot_session-hot": 1
  },
  "inbound_seen_files": {}
}
"#,
    )
    .expect("write state");

    let qmd = tmp.path().join("qmd");
    write_fake_qmd_hot_lifecycle_unsupported(&qmd);
    let openclaw = tmp.path().join("openclaw");
    write_fake_openclaw(&openclaw);

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("OPENCLAW_SESSIONS_DIR", &sessions_dir)
        .env("QMD_BIN", &qmd)
        .env("OPENCLAW_BIN", &openclaw)
        .arg("watch")
        .arg("--once")
        .assert()
        .failure()
        .stderr(contains(
            "watcher strict mode hot collection lifecycle failed",
        ));
}
