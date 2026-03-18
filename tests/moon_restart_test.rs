#![cfg(not(windows))]
use predicates::str::contains;
use tempfile::tempdir;

fn prepare_moon_home(path: &std::path::Path) {
    std::fs::create_dir_all(path).expect("mkdir moon home");
    std::fs::write(path.join(".env"), "\n").expect("write moon env");
}

#[test]
fn moon_restart_is_registered_in_help() {
    let tmp = tempdir().expect("tempdir");
    let moon_home = tmp.path().join("moon-home");
    prepare_moon_home(&moon_home);

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("restart"));
}

#[test]
fn moon_restart_runs_stop_before_start_attempt() {
    let tmp = tempdir().expect("tempdir");
    let moon_home = tmp.path().join("moon-home");
    prepare_moon_home(&moon_home);
    let logs_dir = tmp.path().join("moon").join("logs");
    std::fs::create_dir_all(&logs_dir).expect("mkdir logs");

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("MOON_LOGS_DIR", &logs_dir)
        .arg("restart")
        .assert()
        .failure()
        .stdout(contains(
            "CRITICAL: Running the background daemon from a development binary is disabled for stability.",
        ))
        .stdout(contains(
            "Please install the binary to your path first: `cargo install --path .`",
        ));
}
