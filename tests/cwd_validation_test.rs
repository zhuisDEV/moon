use predicates::str::contains;
use std::fs;
use std::path::Path;
use tempfile::tempdir;

fn prepare_moon_home(path: &Path) {
    fs::create_dir_all(path).expect("mkdir moon home");
    fs::write(path.join(".env"), "\n").expect("write moon env");
}

#[test]
fn mutating_commands_fail_outside_explicit_workspace() {
    let workspace = tempdir().expect("workspace tempdir");
    let run_dir = tempdir().expect("run tempdir");
    let moon_home = workspace.path().join("moon-home");
    prepare_moon_home(&moon_home);

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(run_dir.path())
        .env("MOON_HOME", &moon_home)
        .arg("stop")
        .assert()
        .failure()
        .stderr(contains("E004_CWD_INVALID"));
}

#[test]
fn allow_out_of_bounds_bypasses_workspace_validation() {
    let workspace = tempdir().expect("workspace tempdir");
    let run_dir = tempdir().expect("run tempdir");
    let moon_home = workspace.path().join("moon-home");
    prepare_moon_home(&moon_home);

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(run_dir.path())
        .env("MOON_HOME", &moon_home)
        .args(["--allow-out-of-bounds", "stop"])
        .assert()
        .success();
}

#[test]
fn env_allow_out_of_bounds_bypasses_workspace_validation() {
    let workspace = tempdir().expect("workspace tempdir");
    let run_dir = tempdir().expect("run tempdir");
    let moon_home = workspace.path().join("moon-home");
    prepare_moon_home(&moon_home);

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(run_dir.path())
        .env("MOON_HOME", &moon_home)
        .env("MOON_ALLOW_OUT_OF_BOUNDS", "1")
        .arg("stop")
        .assert()
        .success();
}
