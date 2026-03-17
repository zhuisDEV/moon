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

#[cfg(unix)]
#[test]
fn install_writes_canonical_plugin_and_runtime_paths_from_symlinked_roots() {
    use std::os::unix::fs::symlink;

    let tmp = tempdir().expect("tempdir");
    let real_root = tmp.path().join("real");
    fs::create_dir_all(&real_root).expect("mkdir real");

    let alias_root = tmp.path().join("alias");
    symlink(&real_root, &alias_root).expect("symlink alias");

    let state_dir = alias_root.join("state");
    fs::create_dir_all(&state_dir).expect("mkdir state");
    let moon_home = alias_root.join("moon-home");
    fs::create_dir_all(&moon_home).expect("mkdir moon home");
    fs::write(moon_home.join(".env"), "\n").expect("write moon .env");
    let config_path = state_dir.join("openclaw.json");
    fs::write(&config_path, "{}\n").expect("write config");

    let fake_openclaw = tmp.path().join("openclaw");
    let log_path = tmp.path().join("openclaw.log");
    write_fake_openclaw(&fake_openclaw, &log_path);

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(&alias_root)
        .env("MOON_HOME", &moon_home)
        .env("OPENCLAW_STATE_DIR", &state_dir)
        .env("OPENCLAW_CONFIG_PATH", &config_path)
        .env("OPENCLAW_BIN", &fake_openclaw)
        .args(["--allow-out-of-bounds", "install"])
        .assert()
        .success();

    let cfg: Value = serde_json::from_str(&fs::read_to_string(&config_path).expect("read config"))
        .expect("parse cfg");
    let expected_plugin_dir = fs::canonicalize(alias_root.join("state/extensions/moon"))
        .expect("canonicalize plugin dir");
    let expected_moon_home =
        fs::canonicalize(alias_root.join("moon-home")).expect("canonicalize moon home");

    assert_eq!(
        cfg.get("plugins")
            .and_then(|v| v.get("installs"))
            .and_then(|v| v.get("moon"))
            .and_then(|v| v.get("sourcePath"))
            .and_then(Value::as_str),
        Some(expected_plugin_dir.to_string_lossy().as_ref())
    );
    assert_eq!(
        cfg.get("plugins")
            .and_then(|v| v.get("installs"))
            .and_then(|v| v.get("moon"))
            .and_then(|v| v.get("installPath"))
            .and_then(Value::as_str),
        Some(expected_plugin_dir.to_string_lossy().as_ref())
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
