use predicates::str::contains;
use serde_json::Value;
use std::fs;
use std::path::Path;
use tempfile::tempdir;

fn write_fake_openclaw(bin_path: &Path, log_path: &Path) {
    let script = format!(
        r#"#!/usr/bin/env bash
set -euo pipefail
echo "$@" >> "{}"
if [ "$1" = "plugins" ] && [ "$2" = "install" ]; then
  src="${{@: -1}}"
  source_real="$(cd "$src" && pwd -P)"
  target="$OPENCLAW_STATE_DIR/extensions/moon"
  rm -rf "$target"
  mkdir -p "$(dirname "$target")"
  cp -R "$source_real" "$target"
  target_real="$(cd "$target" && pwd -P)"
  mkdir -p "$OPENCLAW_STATE_DIR/plugins"
  cat > "$OPENCLAW_STATE_DIR/plugins/installs.json" <<JSON
{{"installRecords":{{"moon":{{"source":"path","sourcePath":"$source_real","installPath":"$target_real"}}}},"plugins":[]}}
JSON
fi
if [ "$1" = "plugins" ] && [ "$2" = "list" ]; then
  echo '{{"plugins":[{{"id":"moon"}}]}}'
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
fn second_install_is_noop_for_plugin_sync() {
    let tmp = tempdir().expect("tempdir");
    let state_dir = tmp.path().join("state");
    let moon_home = tmp.path().join("moon-home");
    fs::create_dir_all(&state_dir).expect("mkdir");
    fs::create_dir_all(&moon_home).expect("mkdir moon home");
    fs::write(moon_home.join(".env"), "\n").expect("write moon .env");
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

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("OPENCLAW_STATE_DIR", &state_dir)
        .env("OPENCLAW_CONFIG_PATH", &config_path)
        .env("OPENCLAW_BIN", &fake_openclaw)
        .arg("install")
        .assert()
        .success()
        .stdout(contains("plugin_changed=false"));

    let cfg: Value = serde_json::from_str(&fs::read_to_string(&config_path).expect("read config"))
        .expect("parse config");
    let install_index: Value = serde_json::from_str(
        &fs::read_to_string(state_dir.join("plugins/installs.json")).expect("read plugin index"),
    )
    .expect("parse plugin index");
    let expected_plugin_source_dir =
        fs::canonicalize(state_dir.join("plugin-sources").join("moon"))
            .expect("canonicalize plugin source dir");
    let expected_plugin_dir = fs::canonicalize(state_dir.join("extensions").join("moon"))
        .expect("canonicalize plugin dir");
    let expected_moon_home = fs::canonicalize(&moon_home).expect("canonicalize moon home");

    assert_eq!(
        install_index
            .get("installRecords")
            .and_then(|v| v.get("moon"))
            .and_then(|v| v.get("source"))
            .and_then(Value::as_str),
        Some("path")
    );
    assert_eq!(
        install_index
            .get("installRecords")
            .and_then(|v| v.get("moon"))
            .and_then(|v| v.get("sourcePath"))
            .and_then(Value::as_str),
        Some(expected_plugin_source_dir.to_string_lossy().as_ref())
    );
    assert_eq!(
        install_index
            .get("installRecords")
            .and_then(|v| v.get("moon"))
            .and_then(|v| v.get("installPath"))
            .and_then(Value::as_str),
        Some(expected_plugin_dir.to_string_lossy().as_ref())
    );
    assert!(
        cfg.get("plugins").and_then(|v| v.get("installs")).is_none(),
        "install provenance should live in the plugin index, not openclaw.json"
    );
    assert_eq!(
        cfg.get("plugins")
            .and_then(|v| v.get("slots"))
            .and_then(|v| v.get("contextEngine"))
            .and_then(Value::as_str),
        Some("moon")
    );
    assert_eq!(
        cfg.get("plugins")
            .and_then(|v| v.get("entries"))
            .and_then(|v| v.get("moon"))
            .and_then(|v| v.get("config"))
            .and_then(|v| v.get("contextEngineTimeoutMs"))
            .and_then(Value::as_i64),
        Some(120_000)
    );
    assert_eq!(
        cfg.get("plugins")
            .and_then(|v| v.get("entries"))
            .and_then(|v| v.get("moon"))
            .and_then(|v| v.get("config"))
            .and_then(|v| v.get("moonHome"))
            .and_then(Value::as_str),
        Some(expected_moon_home.to_string_lossy().as_ref())
    );
    assert_eq!(
        cfg.get("plugins")
            .and_then(|v| v.get("entries"))
            .and_then(|v| v.get("moon"))
            .and_then(|v| v.get("config"))
            .and_then(|v| v.get("fallbackMode"))
            .and_then(Value::as_str),
        Some("disabled")
    );
}
