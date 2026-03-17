use predicates::str::contains;
use serde_json::Value;
use std::fs;
use std::path::Path;
use tempfile::tempdir;

fn write_fake_openclaw(bin_path: &Path, log_path: &Path, plugins_list_payload: &str) {
    let script = format!(
        r#"#!/usr/bin/env bash
echo "$@" >> "{}"
if [ "$1" = "plugins" ] && [ "$2" = "list" ]; then
  cat <<'JSON'
{}
JSON
fi
exit 0
"#,
        log_path.display(),
        plugins_list_payload
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

fn run_install(temp_root: &Path, state_dir: &Path, config_path: &Path, openclaw_bin: &Path) {
    let moon_home = temp_root.join("moon-home");
    fs::create_dir_all(&moon_home).expect("mkdir moon home");
    fs::write(moon_home.join(".env"), "\n").expect("write moon env");
    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(temp_root)
        .env("MOON_HOME", &moon_home)
        .env("OPENCLAW_STATE_DIR", state_dir)
        .env("OPENCLAW_CONFIG_PATH", config_path)
        .env("OPENCLAW_BIN", openclaw_bin)
        .arg("install")
        .assert()
        .success();
}

#[test]
fn status_fails_when_runtime_reports_untracked_provenance() {
    let tmp = tempdir().expect("tempdir");
    let moon_home = tmp.path().join("moon-home");
    fs::create_dir_all(&moon_home).expect("mkdir moon home");
    fs::write(moon_home.join(".env"), "\n").expect("write moon env");
    let state_dir = tmp.path().join("state");
    fs::create_dir_all(&state_dir).expect("mkdir");
    let config_path = state_dir.join("openclaw.json");
    fs::write(&config_path, "{}\n").expect("write config");

    let fake_openclaw = tmp.path().join("openclaw");
    let log_path = tmp.path().join("openclaw.log");
    let plugins_list_payload = r#"{
  "plugins": [
    {"id":"moon","status":"loaded"}
  ],
  "diagnostics": [
    {
      "level":"warn",
      "pluginId":"moon",
      "source":"/tmp/extensions/moon/index.js",
      "message":"loaded without install/load-path provenance; treat as untracked local code and pin trust via plugins.allow or install records"
    }
  ]
}"#;
    write_fake_openclaw(&fake_openclaw, &log_path, plugins_list_payload);

    run_install(tmp.path(), &state_dir, &config_path, &fake_openclaw);

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("OPENCLAW_STATE_DIR", &state_dir)
        .env("OPENCLAW_CONFIG_PATH", &config_path)
        .env("OPENCLAW_BIN", &fake_openclaw)
        .arg("verify")
        .assert()
        .failure()
        .stdout(contains(
            "plugin loaded without install/load-path provenance",
        ));
}

#[test]
fn status_tolerates_missing_install_record_when_diagnostics_are_clean() {
    let tmp = tempdir().expect("tempdir");
    let moon_home = tmp.path().join("moon-home");
    fs::create_dir_all(&moon_home).expect("mkdir moon home");
    fs::write(moon_home.join(".env"), "\n").expect("write moon env");
    let state_dir = tmp.path().join("state");
    fs::create_dir_all(&state_dir).expect("mkdir");
    let config_path = state_dir.join("openclaw.json");
    fs::write(&config_path, "{}\n").expect("write config");

    let fake_openclaw = tmp.path().join("openclaw");
    let log_path = tmp.path().join("openclaw.log");
    let plugins_list_payload = r#"{
  "plugins": [
    {"id":"moon","status":"loaded"}
  ],
  "diagnostics": []
}"#;
    write_fake_openclaw(&fake_openclaw, &log_path, plugins_list_payload);

    run_install(tmp.path(), &state_dir, &config_path, &fake_openclaw);

    let mut cfg: Value =
        serde_json::from_str(&fs::read_to_string(&config_path).expect("read config"))
            .expect("parse config");
    cfg.get_mut("plugins")
        .and_then(Value::as_object_mut)
        .expect("plugins object")
        .remove("installs");
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
        .success()
        .stdout(contains("provenance repair hint"));
}

#[test]
fn verify_strict_fails_when_runtime_reports_untracked_provenance() {
    let tmp = tempdir().expect("tempdir");
    let moon_home = tmp.path().join("moon-home");
    fs::create_dir_all(&moon_home).expect("mkdir moon home");
    fs::write(moon_home.join(".env"), "\n").expect("write moon env");
    let state_dir = tmp.path().join("state");
    fs::create_dir_all(&state_dir).expect("mkdir");
    let config_path = state_dir.join("openclaw.json");
    fs::write(&config_path, "{}\n").expect("write config");

    let fake_openclaw = tmp.path().join("openclaw");
    let log_path = tmp.path().join("openclaw.log");
    let plugins_list_payload = r#"{
  "plugins": [
    {"id":"moon","status":"loaded"}
  ],
  "diagnostics": [
    {
      "level":"warn",
      "pluginId":"moon",
      "source":"/tmp/extensions/moon/index.js",
      "message":"loaded without install/load-path provenance; treat as untracked local code and pin trust via plugins.allow or install records"
    }
  ]
}"#;
    write_fake_openclaw(&fake_openclaw, &log_path, plugins_list_payload);

    run_install(tmp.path(), &state_dir, &config_path, &fake_openclaw);

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
            "plugin loaded without install/load-path provenance",
        ))
        .stdout(contains("strict verify failed"));
}
