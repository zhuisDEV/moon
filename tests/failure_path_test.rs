use predicates::str::contains;
use std::fs;
use std::path::Path;
use tempfile::tempdir;

fn prepare_moon_home(path: &Path) {
    fs::create_dir_all(path).expect("mkdir moon home");
    fs::write(path.join(".env"), "\n").expect("write moon env");
}

#[test]
fn verify_fails_when_openclaw_binary_missing() {
    let tmp = tempdir().expect("tempdir");
    let state_dir = tmp.path().join("state");
    let moon_home = tmp.path().join("moon-home");
    fs::create_dir_all(&state_dir).expect("mkdir");
    prepare_moon_home(&moon_home);
    let config_path = state_dir.join("openclaw.json");
    fs::write(&config_path, "{}\n").expect("write config");

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("OPENCLAW_STATE_DIR", &state_dir)
        .env("OPENCLAW_CONFIG_PATH", &config_path)
        .env("OPENCLAW_BIN", "/definitely/not/a/real/openclaw")
        .arg("verify")
        .assert()
        .failure()
        .stdout(contains("openclaw binary unavailable"));
}

#[test]
fn install_fails_when_config_invalid() {
    let tmp = tempdir().expect("tempdir");
    let state_dir = tmp.path().join("state");
    let moon_home = tmp.path().join("moon-home");
    fs::create_dir_all(&state_dir).expect("mkdir");
    prepare_moon_home(&moon_home);
    let config_path = state_dir.join("openclaw.json");
    fs::write(&config_path, "{not valid json5 :::").expect("write config");

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("OPENCLAW_STATE_DIR", &state_dir)
        .env("OPENCLAW_CONFIG_PATH", &config_path)
        .arg("install")
        .assert()
        .failure()
        .stderr(contains("failed to parse config as JSON/JSON5"));
}
