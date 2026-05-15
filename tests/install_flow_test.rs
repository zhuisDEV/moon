use serde_json::Value;
use std::fs;
use std::path::Path;
use tempfile::tempdir;

#[cfg(unix)]
fn assert_owner_only(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let mode = fs::metadata(path).expect("metadata").permissions().mode() & 0o777;
    assert_eq!(
        mode & 0o077,
        0,
        "expected owner-only permissions for {} but got {:03o}",
        path.display(),
        mode
    );
}

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
  echo '[{{"id":"moon"}}]'
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
fn install_creates_plugin_and_stage2_config_entries() {
    let tmp = tempdir().expect("tempdir");
    let state_dir = tmp.path().join("state");
    let moon_home = tmp.path().join("moon-home");
    fs::create_dir_all(&state_dir).expect("mkdir");
    fs::create_dir_all(&moon_home).expect("mkdir moon home");
    fs::write(moon_home.join(".env"), "\n").expect("write moon .env");
    fs::write(moon_home.join("BOOTSTRAP.md"), "legacy bootstrap\n")
        .expect("write legacy runtime bootstrap");
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

    let plugin_dir = state_dir.join("extensions").join("moon");
    assert!(plugin_dir.join("index.js").exists());
    assert!(plugin_dir.join("openclaw.plugin.json").exists());
    assert!(plugin_dir.join("package.json").exists());
    assert!(
        !moon_home.join("moon").exists(),
        "install should not create a nested moon dir inside MOON_HOME"
    );
    assert!(moon_home.join("raw").exists());
    assert!(moon_home.join("mds").exists());
    assert!(moon_home.join("cleanse").exists());
    assert!(moon_home.join("memory").exists());
    assert!(moon_home.join("logs").exists());
    assert!(moon_home.join("mce").exists());
    assert!(moon_home.join("state").exists());
    assert!(moon_home.join("MEMORY.md").exists());
    assert!(moon_home.join(".env").exists());
    assert!(moon_home.join("README.md").exists());
    assert!(moon_home.join(".env.example").exists());
    assert!(moon_home.join("moon.toml.example").exists());
    assert!(moon_home.join("docs/troubleshooting.md").exists());
    assert!(
        !moon_home.join("BOOTSTRAP.md").exists(),
        "legacy runtime bootstrap doc should be removed"
    );
    assert!(state_dir.join("skills/moon-admin/SKILL.md").exists());
    assert!(state_dir.join("skills/moon-subagent/SKILL.md").exists());
    let runtime_env = fs::read_to_string(moon_home.join(".env")).expect("read runtime env");
    assert!(
        runtime_env.contains("MOON_CLEANSE_PROVIDER=gemini") || runtime_env.trim().is_empty(),
        "runtime .env should either keep caller-provided content or include bootstrap template"
    );
    #[cfg(unix)]
    {
        assert_owner_only(&moon_home.join(".env"));
        assert_owner_only(&moon_home.join("logs"));
    }

    let cfg: Value = serde_json::from_str(&fs::read_to_string(&config_path).expect("read config"))
        .expect("parse cfg");
    let install_index: Value = serde_json::from_str(
        &fs::read_to_string(state_dir.join("plugins/installs.json")).expect("read plugin index"),
    )
    .expect("parse plugin index");
    let expected_plugin_source_dir =
        fs::canonicalize(state_dir.join("plugin-sources").join("moon"))
            .expect("canonicalize plugin source dir");
    let expected_plugin_dir = fs::canonicalize(&plugin_dir).expect("canonicalize plugin dir");
    let expected_moon_home = fs::canonicalize(&moon_home).expect("canonicalize moon home");
    let expected_memory_dir = expected_moon_home.join("memory");
    let expected_memory_file = expected_moon_home.join("MEMORY.md");
    assert_eq!(
        cfg.get("plugins")
            .and_then(|v| v.get("entries"))
            .and_then(|v| v.get("moon"))
            .and_then(|v| v.get("enabled"))
            .and_then(Value::as_bool),
        Some(true)
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
            .and_then(|v| v.get("slots"))
            .and_then(|v| v.get("memory"))
            .and_then(Value::as_str),
        Some("none")
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
            .and_then(|v| v.get("contextEngineTimeoutMs"))
            .and_then(Value::as_i64),
        Some(120_000)
    );
    assert_eq!(
        cfg.get("plugins")
            .and_then(|v| v.get("entries"))
            .and_then(|v| v.get("moon"))
            .and_then(|v| v.get("config"))
            .and_then(|v| v.get("memoryDir"))
            .and_then(Value::as_str),
        Some(expected_memory_dir.to_string_lossy().as_ref())
    );
    assert_eq!(
        cfg.get("plugins")
            .and_then(|v| v.get("entries"))
            .and_then(|v| v.get("moon"))
            .and_then(|v| v.get("config"))
            .and_then(|v| v.get("memoryFile"))
            .and_then(Value::as_str),
        Some(expected_memory_file.to_string_lossy().as_ref())
    );
    assert!(
        cfg.get("plugins")
            .and_then(|v| v.get("entries"))
            .and_then(|v| v.get("moon"))
            .and_then(|v| v.get("config"))
            .and_then(|v| v.get("moonPath"))
            .and_then(Value::as_str)
            .is_some_and(|path| std::path::Path::new(path).is_file())
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
    assert_eq!(
        cfg.get("plugins")
            .and_then(|v| v.get("entries"))
            .and_then(|v| v.get("moon"))
            .and_then(|v| v.get("config"))
            .and_then(|v| v.get("compactFallbackOnSkip"))
            .and_then(Value::as_bool),
        Some(false)
    );

    assert_eq!(
        cfg.get("plugins")
            .and_then(|v| v.get("entries"))
            .and_then(|v| v.get("moon"))
            .and_then(|v| v.get("config"))
            .and_then(|v| v.get("maxTokens"))
            .and_then(Value::as_i64),
        Some(12_000)
    );

    assert_eq!(
        cfg.get("plugins")
            .and_then(|v| v.get("entries"))
            .and_then(|v| v.get("moon"))
            .and_then(|v| v.get("config"))
            .and_then(|v| v.get("tools"))
            .and_then(|v| v.get("read"))
            .and_then(|v| v.get("maxTokens"))
            .and_then(Value::as_i64),
        Some(6_000)
    );
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

    assert!(
        cfg.get("agents")
            .and_then(|v| v.get("defaults"))
            .and_then(|v| v.get("contextPruning"))
            .is_none()
    );
    assert_eq!(
        cfg.get("agents")
            .and_then(|v| v.get("defaults"))
            .and_then(|v| v.get("memorySearch"))
            .and_then(|v| v.get("enabled"))
            .and_then(Value::as_bool),
        Some(false)
    );
}

#[test]
fn install_followed_by_strict_verify_passes_with_moon_owned_memory_contract() {
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
        .args(["verify", "--strict"])
        .assert()
        .success();
}

#[test]
fn install_does_not_create_nested_moon_dir_under_moon_home() {
    let tmp = tempdir().expect("tempdir");
    let state_dir = tmp.path().join("state");
    let moon_home = tmp.path().join("moon-home");
    fs::create_dir_all(&state_dir).expect("mkdir state");
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

    assert!(
        !moon_home.join("moon").exists(),
        "install should never create $MOON_HOME/moon"
    );
}
