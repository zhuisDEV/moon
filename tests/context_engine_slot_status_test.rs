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
