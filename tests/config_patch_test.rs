use serde_json::Value;
use std::fs;
use std::path::Path;
use tempfile::tempdir;

fn write_fake_openclaw(bin_path: &Path, log_path: &Path) {
    let script = format!(
        "#!/usr/bin/env bash\necho \"$@\" >> \"{}\"\nif [ \"$1\" = \"plugins\" ] && [ \"$2\" = \"list\" ]; then\n  echo '[{{\"id\":\"moon\"}}]'\nfi\nexit 0\n",
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
fn patch_respects_existing_values_unless_forced() {
    let tmp = tempdir().expect("tempdir");
    let state_dir = tmp.path().join("state");
    let moon_home = tmp.path().join("moon-home");
    fs::create_dir_all(&state_dir).expect("mkdir");
    fs::create_dir_all(&moon_home).expect("mkdir moon home");
    fs::write(moon_home.join(".env"), "\n").expect("write moon .env");
    let config_path = state_dir.join("openclaw.json");

    fs::write(
        &config_path,
        r#"{
  "agents": {"defaults": {"compaction": {"reserveTokensFloor": 123}}},
  "plugins": {
    "slots": {"memory": "memory-core"},
    "entries": {
      "moon": {
        "config": {
          "maxTokens": 999
        }
      }
    }
  }
}"#,
    )
    .expect("write config");

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

    let cfg_1: Value =
        serde_json::from_str(&fs::read_to_string(&config_path).expect("read config"))
            .expect("parse cfg");
    let expected_moon_home = fs::canonicalize(&moon_home).expect("canonicalize moon home");
    assert_eq!(
        cfg_1
            .get("agents")
            .and_then(|v| v.get("defaults"))
            .and_then(|v| v.get("compaction"))
            .and_then(|v| v.get("reserveTokensFloor"))
            .and_then(Value::as_i64),
        Some(123)
    );
    assert_eq!(
        cfg_1
            .get("plugins")
            .and_then(|v| v.get("slots"))
            .and_then(|v| v.get("contextEngine"))
            .and_then(Value::as_str),
        Some("moon")
    );
    assert_eq!(
        cfg_1
            .get("plugins")
            .and_then(|v| v.get("slots"))
            .and_then(|v| v.get("memory"))
            .and_then(Value::as_str),
        Some("none")
    );
    assert_eq!(
        cfg_1
            .get("plugins")
            .and_then(|v| v.get("entries"))
            .and_then(|v| v.get("moon"))
            .and_then(|v| v.get("config"))
            .and_then(|v| v.get("moonHome"))
            .and_then(Value::as_str),
        Some(expected_moon_home.to_string_lossy().as_ref())
    );
    assert_eq!(
        cfg_1
            .get("plugins")
            .and_then(|v| v.get("entries"))
            .and_then(|v| v.get("moon"))
            .and_then(|v| v.get("config"))
            .and_then(|v| v.get("fallbackMode"))
            .and_then(Value::as_str),
        Some("disabled")
    );
    assert_eq!(
        cfg_1
            .get("plugins")
            .and_then(|v| v.get("entries"))
            .and_then(|v| v.get("moon"))
            .and_then(|v| v.get("config"))
            .and_then(|v| v.get("maxTokens"))
            .and_then(Value::as_i64),
        Some(999)
    );
    assert_eq!(
        cfg_1
            .get("agents")
            .and_then(|v| v.get("defaults"))
            .and_then(|v| v.get("memorySearch"))
            .and_then(|v| v.get("enabled"))
            .and_then(Value::as_bool),
        Some(false)
    );

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("OPENCLAW_STATE_DIR", &state_dir)
        .env("OPENCLAW_CONFIG_PATH", &config_path)
        .env("OPENCLAW_BIN", &fake_openclaw)
        .args(["install", "--force"])
        .assert()
        .success();

    let cfg_2: Value =
        serde_json::from_str(&fs::read_to_string(&config_path).expect("read config"))
            .expect("parse cfg");
    assert_eq!(
        cfg_2
            .get("agents")
            .and_then(|v| v.get("defaults"))
            .and_then(|v| v.get("compaction"))
            .and_then(|v| v.get("reserveTokensFloor"))
            .and_then(Value::as_i64),
        Some(123)
    );
    assert_eq!(
        cfg_2
            .get("plugins")
            .and_then(|v| v.get("entries"))
            .and_then(|v| v.get("moon"))
            .and_then(|v| v.get("config"))
            .and_then(|v| v.get("maxTokens"))
            .and_then(Value::as_i64),
        Some(12_000)
    );
    assert_eq!(
        cfg_2
            .get("agents")
            .and_then(|v| v.get("defaults"))
            .and_then(|v| v.get("memorySearch"))
            .and_then(|v| v.get("enabled"))
            .and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        cfg_2
            .get("plugins")
            .and_then(|v| v.get("slots"))
            .and_then(|v| v.get("memory"))
            .and_then(Value::as_str),
        Some("none")
    );
}
