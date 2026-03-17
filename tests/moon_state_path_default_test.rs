#![cfg(not(windows))]
use predicates::str::contains;
use tempfile::tempdir;

#[test]
fn status_default_state_path_uses_state_subdir_when_moon_home_is_set() {
    let tmp = tempdir().expect("tempdir");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).expect("mkdir home");
    let moon_home = home.join(".moon");
    std::fs::create_dir_all(&moon_home).expect("mkdir moon home");
    std::fs::write(moon_home.join(".env"), "\n").expect("write moon env");

    let expected = moon_home.join("state/moon_state.json");

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("HOME", &home)
        .env("MOON_HOME", &moon_home)
        .env_remove("MOON_STATE_FILE")
        .env_remove("MOON_STATE_DIR")
        .arg("status")
        .assert()
        .failure()
        .stdout(contains(format!("state_file={}", expected.display())));
}
