use predicates::str::contains;
use std::fs;
use tempfile::tempdir;

#[test]
fn help_loads_env_from_default_home_when_moon_home_is_unset() {
    let tmp = tempdir().expect("tempdir");
    let home = tmp.path().join("home");
    let moon_home = home.join(".moon");

    fs::create_dir_all(&moon_home).expect("mkdir moon home");
    fs::write(moon_home.join(".env"), "\n").expect("write moon env");

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("HOME", &home)
        .env_remove("MOON_HOME")
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("Usage: moon"));
}
