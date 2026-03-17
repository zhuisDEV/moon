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
fn install_creates_plugin_and_stage2_config_entries() {
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

    let plugin_dir = state_dir.join("extensions").join("moon");
    assert!(plugin_dir.join("index.js").exists());
    assert!(plugin_dir.join("openclaw.plugin.json").exists());
    assert!(plugin_dir.join("package.json").exists());
    assert!(moon_home.join("raw").exists());
    assert!(moon_home.join("mds").exists());
    assert!(moon_home.join("cleanse").exists());
    assert!(moon_home.join("archives").exists());
    assert!(moon_home.join("memory").exists());
    assert!(moon_home.join("logs").exists());
    assert!(moon_home.join("mce").exists());
    assert!(moon_home.join("state").exists());
    assert!(moon_home.join("MEMORY.md").exists());
    assert!(moon_home.join(".env").exists());
    assert!(moon_home.join("README.md").exists());
    assert!(moon_home.join("BOOTSTRAP.md").exists());
    assert!(moon_home.join(".env.example").exists());
    assert!(moon_home.join("moon.toml.example").exists());
    assert!(moon_home.join("docs/troubleshooting.md").exists());
    assert!(state_dir.join("skills/moon-admin/SKILL.md").exists());
    assert!(state_dir.join("skills/moon-subagent/SKILL.md").exists());
    let runtime_env = fs::read_to_string(moon_home.join(".env")).expect("read runtime env");
    assert!(
        runtime_env.contains("MOON_CLEANSE_PROVIDER=gemini") || runtime_env.trim().is_empty(),
        "runtime .env should either keep caller-provided content or include bootstrap template"
    );

    let cfg: Value = serde_json::from_str(&fs::read_to_string(&config_path).expect("read config"))
        .expect("parse cfg");
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
        cfg.get("plugins")
            .and_then(|v| v.get("installs"))
            .and_then(|v| v.get("moon"))
            .and_then(|v| v.get("source"))
            .and_then(Value::as_str),
        Some("path")
    );
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

    assert!(
        cfg.get("agents")
            .and_then(|v| v.get("defaults"))
            .and_then(|v| v.get("contextPruning"))
            .is_none()
    );
    assert_eq!(
        cfg.get("agents")
            .and_then(|v| v.get("defaults"))
            .and_then(|v| v.get("contextTokens"))
            .and_then(Value::as_i64),
        None
    );
}
