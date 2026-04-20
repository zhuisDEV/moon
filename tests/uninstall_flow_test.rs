use predicates::str::contains;
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use tempfile::tempdir;

fn write_executable(path: &Path, content: &str) {
    fs::write(path, content).expect("write executable");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod");
    }
}

fn write_fake_openclaw(bin_path: &Path, log_path: &Path) {
    write_executable(
        bin_path,
        &format!(
            "#!/usr/bin/env bash\nset -euo pipefail\necho \"$@\" >> \"{}\"\nexit 0\n",
            log_path.display()
        ),
    );
}

fn prepare_runtime_tree(moon_home: &Path) {
    fs::create_dir_all(moon_home.join("raw")).expect("mkdir raw");
    fs::create_dir_all(moon_home.join("mds")).expect("mkdir mds");
    fs::create_dir_all(moon_home.join("mlib")).expect("mkdir mlib");
    fs::create_dir_all(moon_home.join("cleanse")).expect("mkdir cleanse");
    fs::create_dir_all(moon_home.join("logs")).expect("mkdir logs");
    fs::create_dir_all(moon_home.join("mce")).expect("mkdir mce");
    fs::create_dir_all(moon_home.join("mcp")).expect("mkdir packets");
    fs::create_dir_all(moon_home.join("docs")).expect("mkdir docs");
    fs::create_dir_all(moon_home.join("state")).expect("mkdir state");
    fs::create_dir_all(moon_home.join("qmd")).expect("mkdir qmd");
    fs::create_dir_all(moon_home.join("memory")).expect("mkdir memory");
    fs::create_dir_all(moon_home.join("auth")).expect("mkdir auth");
    fs::write(moon_home.join(".env"), "MOON_HOME=test\n").expect("write env");
    fs::write(
        moon_home.join("moon.toml"),
        "[context]\nwindow_mode=\"fixed\"\n",
    )
    .expect("write config");
    fs::write(moon_home.join("README.md"), "runtime docs\n").expect("write runtime readme");
    fs::write(moon_home.join(".env.example"), "example\n").expect("write env example");
    fs::write(moon_home.join("moon.toml.example"), "example\n").expect("write toml example");
    fs::write(
        moon_home.join("docs/troubleshooting.md"),
        "troubleshooting\n",
    )
    .expect("write troubleshooting");
    fs::write(moon_home.join("memory/2026-04-20.md"), "daily memory\n")
        .expect("write daily memory");
    fs::write(moon_home.join("MEMORY.md"), "durable memory\n").expect("write memory file");
    fs::write(moon_home.join("auth/openai-codex.json"), "{}\n").expect("write auth");
}

fn write_openclaw_config(config_path: &Path) {
    let payload = json!({
        "plugins": {
            "entries": {
                "moon": {
                    "enabled": true,
                    "config": {
                        "moonPath": "/tmp/moon",
                        "moonHome": "/tmp/moon-home"
                    }
                }
            },
            "installs": {
                "moon": {
                    "source": "path",
                    "sourcePath": "/tmp/openclaw/extensions/moon",
                    "installPath": "/tmp/openclaw/extensions/moon"
                }
            },
            "slots": {
                "contextEngine": "moon",
                "memory": "none"
            }
        },
        "agents": {
            "defaults": {
                "memorySearch": {
                    "enabled": false
                }
            }
        },
        "channels": {
            "slack": {
                "historyLimit": 10
            }
        }
    });
    fs::write(
        config_path,
        serde_json::to_string_pretty(&payload).expect("serialize config"),
    )
    .expect("write config");
}

#[test]
fn uninstall_removes_integration_and_preserves_user_memory_by_default() {
    let tmp = tempdir().expect("tempdir");
    let home_dir = tmp.path().join("home");
    let moon_home = tmp.path().join("moon-home");
    let state_dir = tmp.path().join("openclaw-state");
    let config_path = state_dir.join("openclaw.json");
    let plugin_dir = state_dir.join("extensions/moon");
    let skills_root = state_dir.join("skills");
    let fake_openclaw = tmp.path().join("openclaw");
    let openclaw_log = tmp.path().join("openclaw.log");

    fs::create_dir_all(&home_dir).expect("mkdir home");
    fs::create_dir_all(&plugin_dir).expect("mkdir plugin dir");
    fs::create_dir_all(skills_root.join("moon-admin")).expect("mkdir admin skill");
    fs::create_dir_all(skills_root.join("moon-subagent")).expect("mkdir subagent skill");
    fs::create_dir_all(&state_dir).expect("mkdir state");
    prepare_runtime_tree(&moon_home);
    write_openclaw_config(&config_path);
    write_fake_openclaw(&fake_openclaw, &openclaw_log);
    fs::write(plugin_dir.join("index.js"), "plugin\n").expect("write plugin");
    fs::write(skills_root.join("moon-admin/SKILL.md"), "admin\n").expect("write admin skill");
    fs::write(skills_root.join("moon-subagent/SKILL.md"), "sub\n").expect("write sub skill");
    fs::write(
        home_dir.join(".zprofile"),
        "export PATH=\"$HOME/bin:$PATH\"\n# Moon runtime home\nexport MOON_HOME=\"${MOON_HOME:-$HOME/.moon}\"\n",
    )
    .expect("write zprofile");

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("HOME", &home_dir)
        .env("MOON_HOME", &moon_home)
        .env("OPENCLAW_STATE_DIR", &state_dir)
        .env("OPENCLAW_CONFIG_PATH", &config_path)
        .env("OPENCLAW_BIN", &fake_openclaw)
        .arg("uninstall")
        .assert()
        .success()
        .stdout(contains("openclaw.plugins.uninstall=ok id=moon"));

    assert!(!plugin_dir.exists(), "plugin dir should be removed");
    assert!(
        !skills_root.join("moon-admin").exists(),
        "runtime admin skill should be removed"
    );
    assert!(
        !skills_root.join("moon-subagent").exists(),
        "runtime subagent skill should be removed"
    );
    assert!(!moon_home.join("raw").exists(), "raw dir should be removed");
    assert!(!moon_home.join("mds").exists(), "mds dir should be removed");
    assert!(
        !moon_home.join("mce").exists(),
        "assembly dir should be removed"
    );
    assert!(
        !moon_home.join("mcp").exists(),
        "context packet dir should be removed"
    );
    assert!(
        !moon_home.join("docs").exists(),
        "runtime docs should be removed"
    );
    assert!(
        moon_home.join("memory").exists(),
        "memory dir should be preserved"
    );
    assert!(
        moon_home.join("MEMORY.md").exists(),
        "memory file should be preserved"
    );
    assert!(moon_home.join(".env").exists(), "env should be preserved");
    assert!(
        moon_home.join("moon.toml").exists(),
        "config should be preserved"
    );
    assert!(moon_home.join("auth").exists(), "auth should be preserved");

    let cfg: Value = serde_json::from_str(&fs::read_to_string(&config_path).expect("read config"))
        .expect("parse cfg");
    assert!(
        cfg.get("plugins")
            .and_then(|v| v.get("entries"))
            .and_then(|v| v.get("moon"))
            .is_none(),
        "moon plugin entry should be removed"
    );
    assert!(
        cfg.get("plugins")
            .and_then(|v| v.get("installs"))
            .and_then(|v| v.get("moon"))
            .is_none(),
        "moon install record should be removed"
    );
    assert!(
        cfg.get("plugins")
            .and_then(|v| v.get("slots"))
            .and_then(|v| v.get("contextEngine"))
            .is_none(),
        "contextEngine slot should be removed"
    );
    assert!(
        cfg.get("plugins")
            .and_then(|v| v.get("slots"))
            .and_then(|v| v.get("memory"))
            .is_none(),
        "moon-owned memory slot should be removed"
    );
    assert!(
        cfg.get("agents")
            .and_then(|v| v.get("defaults"))
            .and_then(|v| v.get("memorySearch"))
            .and_then(|v| v.get("enabled"))
            .is_none(),
        "moon-owned memory search override should be removed"
    );

    let zprofile = fs::read_to_string(home_dir.join(".zprofile")).expect("read zprofile");
    assert!(!zprofile.contains("# Moon runtime home"));
    assert!(!zprofile.contains("MOON_HOME"));

    let log = fs::read_to_string(&openclaw_log).expect("read openclaw log");
    assert!(log.contains("plugins uninstall moon"));
}

#[test]
fn uninstall_purge_removes_entire_moon_home() {
    let tmp = tempdir().expect("tempdir");
    let home_dir = tmp.path().join("home");
    let moon_home = tmp.path().join("moon-home");
    let state_dir = tmp.path().join("openclaw-state");
    let config_path = state_dir.join("openclaw.json");
    let fake_openclaw = tmp.path().join("openclaw");
    let openclaw_log = tmp.path().join("openclaw.log");

    fs::create_dir_all(&home_dir).expect("mkdir home");
    fs::create_dir_all(&state_dir).expect("mkdir state");
    prepare_runtime_tree(&moon_home);
    write_openclaw_config(&config_path);
    write_fake_openclaw(&fake_openclaw, &openclaw_log);
    fs::write(
        home_dir.join(".zprofile"),
        "# Moon runtime home\nexport MOON_HOME=\"${MOON_HOME:-$HOME/.moon}\"\n",
    )
    .expect("write zprofile");

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("HOME", &home_dir)
        .env("MOON_HOME", &moon_home)
        .env("OPENCLAW_STATE_DIR", &state_dir)
        .env("OPENCLAW_CONFIG_PATH", &config_path)
        .env("OPENCLAW_BIN", &fake_openclaw)
        .args(["uninstall", "--purge"])
        .assert()
        .success();

    assert!(!moon_home.exists(), "purge should remove full moon home");
}

#[test]
fn uninstall_is_registered_in_help() {
    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("uninstall"));
}
