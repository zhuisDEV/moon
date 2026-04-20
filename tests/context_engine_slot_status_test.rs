use predicates::str::contains;
use serde_json::Value;
use std::fs;
use std::path::Path;
use tempfile::tempdir;

fn write_fake_openclaw(bin_path: &Path, log_path: &Path) {
    let script = format!(
        r#"#!/usr/bin/env bash
echo "$@" >> "{}"
if [ "$1" = "plugins" ] && [ "$2" = "list" ]; then
  cat <<'JSON'
{{"plugins":[{{"id":"moon","status":"loaded"}}],"diagnostics":[]}}
JSON
fi
exit 0
"#,
        log_path.display()
    );
    fs::write(bin_path, script).expect("write fake openclaw");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(bin_path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(bin_path, perms).expect("chmod");
    }
}

fn write_fake_openclaw_doctor_failure(bin_path: &Path, log_path: &Path) {
    let script = format!(
        r#"#!/usr/bin/env bash
echo "$@" >> "{}"
if [ "$1" = "doctor" ] && [ "$2" = "--non-interactive" ]; then
  echo "non-interactive doctor failed" >&2
  exit 1
fi
if [ "$1" = "plugins" ] && [ "$2" = "info" ] && [ "$3" = "moon" ]; then
  cat <<'JSON'
{{"plugin":{{"id":"moon","status":"loaded"}},"diagnostics":[]}}
JSON
  exit 0
fi
if [ "$1" = "plugins" ] && [ "$2" = "list" ]; then
  cat <<'JSON'
{{"plugins":[{{"id":"moon","status":"loaded"}}],"diagnostics":[]}}
JSON
  exit 0
fi
exit 0
"#,
        log_path.display()
    );
    fs::write(bin_path, script).expect("write fake openclaw");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(bin_path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(bin_path, perms).expect("chmod");
    }
}

fn write_fake_openclaw_doctor_timeout(bin_path: &Path, log_path: &Path) {
    let script = format!(
        r#"#!/usr/bin/env bash
echo "$@" >> "{}"
if [ "$1" = "doctor" ] && [ "$2" = "--non-interactive" ]; then
  sleep 2
  exit 0
fi
if [ "$1" = "plugins" ] && [ "$2" = "info" ] && [ "$3" = "moon" ]; then
  cat <<'JSON'
{{"plugin":{{"id":"moon","status":"loaded"}},"diagnostics":[]}}
JSON
  exit 0
fi
if [ "$1" = "plugins" ] && [ "$2" = "list" ]; then
  cat <<'JSON'
{{"plugins":[{{"id":"moon","status":"loaded"}}],"diagnostics":[]}}
JSON
  exit 0
fi
exit 0
"#,
        log_path.display()
    );
    fs::write(bin_path, script).expect("write fake openclaw");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(bin_path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(bin_path, perms).expect("chmod");
    }
}

#[test]
fn status_fails_when_context_engine_slot_is_not_moon() {
    let tmp = tempdir().expect("tempdir");
    let state_dir = tmp.path().join("state");
    let moon_home = tmp.path().join("moon-home");
    fs::create_dir_all(&state_dir).expect("mkdir");
    fs::create_dir_all(&moon_home).expect("mkdir moon home");
    fs::write(moon_home.join(".env"), "\n").expect("write moon env");
    let config_path = state_dir.join("openclaw.json");
    fs::write(&config_path, "{}\n").expect("write config");

    let fake_openclaw = tmp.path().join("openclaw");
    let log_path = tmp.path().join("openclaw.log");
    write_fake_openclaw(&fake_openclaw, &log_path);

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("OPENCLAW_STATE_DIR", &state_dir)
        .env("OPENCLAW_CONFIG_PATH", &config_path)
        .env("OPENCLAW_BIN", &fake_openclaw)
        .arg("install")
        .assert()
        .success();

    let mut cfg: Value =
        serde_json::from_str(&fs::read_to_string(&config_path).expect("read config"))
            .expect("parse config");
    cfg.get_mut("plugins")
        .and_then(Value::as_object_mut)
        .and_then(|plugins| plugins.get_mut("slots"))
        .and_then(Value::as_object_mut)
        .expect("plugins.slots object")
        .insert("contextEngine".to_string(), Value::from("other"));
    fs::write(
        &config_path,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&cfg).expect("serialize config")
        ),
    )
    .expect("write config");

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("OPENCLAW_STATE_DIR", &state_dir)
        .env("OPENCLAW_CONFIG_PATH", &config_path)
        .env("OPENCLAW_BIN", &fake_openclaw)
        .arg("verify")
        .assert()
        .failure()
        .stdout(contains("plugins.slots.contextEngine must select moon"));
}

#[test]
fn verify_strict_fails_when_memory_contract_is_stale() {
    let tmp = tempdir().expect("tempdir");
    let state_dir = tmp.path().join("state");
    let moon_home = tmp.path().join("moon-home");
    fs::create_dir_all(&state_dir).expect("mkdir");
    fs::create_dir_all(&moon_home).expect("mkdir moon home");
    fs::write(moon_home.join(".env"), "\n").expect("write moon env");
    let config_path = state_dir.join("openclaw.json");
    fs::write(&config_path, "{}\n").expect("write config");

    let fake_openclaw = tmp.path().join("openclaw");
    let log_path = tmp.path().join("openclaw.log");
    write_fake_openclaw(&fake_openclaw, &log_path);

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("OPENCLAW_STATE_DIR", &state_dir)
        .env("OPENCLAW_CONFIG_PATH", &config_path)
        .env("OPENCLAW_BIN", &fake_openclaw)
        .arg("install")
        .assert()
        .success();

    let mut cfg: Value =
        serde_json::from_str(&fs::read_to_string(&config_path).expect("read config"))
            .expect("parse config");
    cfg.get_mut("plugins")
        .and_then(Value::as_object_mut)
        .and_then(|plugins| plugins.get_mut("slots"))
        .and_then(Value::as_object_mut)
        .expect("plugins.slots object")
        .insert("memory".to_string(), Value::from("memory-core"));
    cfg.get_mut("agents")
        .and_then(Value::as_object_mut)
        .and_then(|agents| agents.get_mut("defaults"))
        .and_then(Value::as_object_mut)
        .and_then(|defaults| defaults.get_mut("memorySearch"))
        .and_then(Value::as_object_mut)
        .expect("agents.defaults.memorySearch object")
        .insert("enabled".to_string(), Value::from(true));
    fs::write(
        &config_path,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&cfg).expect("serialize config")
        ),
    )
    .expect("write config");

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("OPENCLAW_STATE_DIR", &state_dir)
        .env("OPENCLAW_CONFIG_PATH", &config_path)
        .env("OPENCLAW_BIN", &fake_openclaw)
        .args(["verify", "--strict"])
        .assert()
        .failure()
        .stdout(contains(
            "plugins.slots.memory expected none, found memory-core",
        ))
        .stdout(contains(
            "agents.defaults.memorySearch.enabled expected false, found true",
        ))
        .stdout(contains("strict verify failed"));
}

#[test]
fn verify_json_reports_doctor_failure_without_interactive_fallback() {
    let tmp = tempdir().expect("tempdir");
    let state_dir = tmp.path().join("state");
    let moon_home = tmp.path().join("moon-home");
    fs::create_dir_all(&state_dir).expect("mkdir");
    fs::create_dir_all(&moon_home).expect("mkdir moon home");
    fs::write(moon_home.join(".env"), "\n").expect("write moon env");
    let config_path = state_dir.join("openclaw.json");
    fs::write(&config_path, "{}\n").expect("write config");

    let fake_openclaw = tmp.path().join("openclaw");
    let log_path = tmp.path().join("openclaw.log");
    write_fake_openclaw_doctor_failure(&fake_openclaw, &log_path);

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("OPENCLAW_STATE_DIR", &state_dir)
        .env("OPENCLAW_CONFIG_PATH", &config_path)
        .env("OPENCLAW_BIN", &fake_openclaw)
        .arg("install")
        .assert()
        .success();

    let assert = assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("OPENCLAW_STATE_DIR", &state_dir)
        .env("OPENCLAW_CONFIG_PATH", &config_path)
        .env("OPENCLAW_BIN", &fake_openclaw)
        .arg("verify")
        .arg("--strict")
        .arg("--json")
        .assert()
        .failure();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    let json: Value = serde_json::from_str(&stdout).expect("parse verify json");
    assert_eq!(json.get("command").and_then(Value::as_str), Some("verify"));
    assert_eq!(json.get("ok").and_then(Value::as_bool), Some(false));
    let issues = json
        .get("issues")
        .and_then(Value::as_array)
        .expect("issues array");
    assert!(
        issues.iter().any(|value| {
            value
                .as_str()
                .is_some_and(|issue| issue.contains("doctor failed:"))
        }),
        "issues did not include doctor failure: {stdout}"
    );
    assert!(
        issues
            .iter()
            .any(|value| value.as_str() == Some("strict verify failed")),
        "issues did not include strict failure marker: {stdout}"
    );

    let log = fs::read_to_string(&log_path).expect("read openclaw log");
    assert!(
        log.lines()
            .any(|line| line.trim() == "doctor --non-interactive"),
        "expected non-interactive doctor invocation in log: {log}"
    );
    assert!(
        !log.lines().any(|line| line.trim() == "doctor"),
        "interactive doctor fallback should not run: {log}"
    );
}

#[test]
fn verify_strict_treats_doctor_timeout_as_advisory_when_status_is_clean() {
    let tmp = tempdir().expect("tempdir");
    let state_dir = tmp.path().join("state");
    let moon_home = tmp.path().join("moon-home");
    fs::create_dir_all(&state_dir).expect("mkdir");
    fs::create_dir_all(&moon_home).expect("mkdir moon home");
    fs::write(moon_home.join(".env"), "\n").expect("write moon env");
    let config_path = state_dir.join("openclaw.json");
    fs::write(&config_path, "{}\n").expect("write config");

    let fake_openclaw = tmp.path().join("openclaw");
    let log_path = tmp.path().join("openclaw.log");
    write_fake_openclaw_doctor_timeout(&fake_openclaw, &log_path);

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("OPENCLAW_STATE_DIR", &state_dir)
        .env("OPENCLAW_CONFIG_PATH", &config_path)
        .env("OPENCLAW_BIN", &fake_openclaw)
        .arg("install")
        .assert()
        .success();

    let assert = assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("OPENCLAW_STATE_DIR", &state_dir)
        .env("OPENCLAW_CONFIG_PATH", &config_path)
        .env("OPENCLAW_BIN", &fake_openclaw)
        .env("MOON_OPENCLAW_DOCTOR_TIMEOUT_SECS", "1")
        .arg("verify")
        .arg("--strict")
        .arg("--json")
        .assert()
        .success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    let json: Value = serde_json::from_str(&stdout).expect("parse verify json");
    assert_eq!(json.get("command").and_then(Value::as_str), Some("verify"));
    assert_eq!(json.get("ok").and_then(Value::as_bool), Some(true));
    let details = json
        .get("details")
        .and_then(Value::as_array)
        .expect("details array");
    assert!(
        details.iter().any(|value| {
            value.as_str().is_some_and(|detail| {
                detail.contains("doctor: timeout_advisory") && detail.contains("timeout_secs=1")
            })
        }),
        "details did not include doctor timeout advisory: {stdout}"
    );
    let issues = json
        .get("issues")
        .and_then(Value::as_array)
        .expect("issues array");
    assert!(issues.is_empty(), "issues should be empty: {stdout}");

    let log = fs::read_to_string(&log_path).expect("read openclaw log");
    assert!(
        log.lines()
            .any(|line| line.trim() == "doctor --non-interactive"),
        "expected non-interactive doctor invocation in log: {log}"
    );
    assert!(
        !log.lines().any(|line| line.trim() == "doctor"),
        "interactive doctor fallback should not run: {log}"
    );
}

#[test]
fn verify_strict_fails_when_memory_contract_is_missing() {
    let tmp = tempdir().expect("tempdir");
    let state_dir = tmp.path().join("state");
    let moon_home = tmp.path().join("moon-home");
    fs::create_dir_all(&state_dir).expect("mkdir");
    fs::create_dir_all(&moon_home).expect("mkdir moon home");
    fs::write(moon_home.join(".env"), "\n").expect("write moon env");
    let config_path = state_dir.join("openclaw.json");
    fs::write(&config_path, "{}\n").expect("write config");

    let fake_openclaw = tmp.path().join("openclaw");
    let log_path = tmp.path().join("openclaw.log");
    write_fake_openclaw(&fake_openclaw, &log_path);

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("OPENCLAW_STATE_DIR", &state_dir)
        .env("OPENCLAW_CONFIG_PATH", &config_path)
        .env("OPENCLAW_BIN", &fake_openclaw)
        .arg("install")
        .assert()
        .success();

    let mut cfg: Value =
        serde_json::from_str(&fs::read_to_string(&config_path).expect("read config"))
            .expect("parse config");
    cfg.get_mut("plugins")
        .and_then(Value::as_object_mut)
        .and_then(|plugins| plugins.get_mut("slots"))
        .and_then(Value::as_object_mut)
        .expect("plugins.slots object")
        .remove("memory");
    cfg.get_mut("agents")
        .and_then(Value::as_object_mut)
        .and_then(|agents| agents.get_mut("defaults"))
        .and_then(Value::as_object_mut)
        .and_then(|defaults| defaults.get_mut("memorySearch"))
        .and_then(Value::as_object_mut)
        .expect("agents.defaults.memorySearch object")
        .remove("enabled");
    fs::write(
        &config_path,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&cfg).expect("serialize config")
        ),
    )
    .expect("write config");

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("OPENCLAW_STATE_DIR", &state_dir)
        .env("OPENCLAW_CONFIG_PATH", &config_path)
        .env("OPENCLAW_BIN", &fake_openclaw)
        .args(["verify", "--strict"])
        .assert()
        .failure()
        .stdout(contains(
            "plugins.slots.memory missing; expected none in a Moon-owned install",
        ))
        .stdout(contains(
            "agents.defaults.memorySearch.enabled missing; expected false in a Moon-owned install",
        ))
        .stdout(contains("strict verify failed"));
}
