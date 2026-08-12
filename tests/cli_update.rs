use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn update_help_exposes_only_the_approved_first_release_interface() {
    Command::cargo_bin("moon")
        .expect("binary")
        .args(["update", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--check"))
        .stdout(predicate::str::contains("--version"))
        .stdout(predicate::str::contains("--dry-run"))
        .stdout(predicate::str::contains("--yes"))
        .stdout(predicate::str::contains("--allow-downgrade"))
        .stdout(predicate::str::contains("--no-restart").not());
}

#[test]
fn invalid_update_mode_is_rejected_before_network_or_storage() {
    let temp = tempfile::tempdir().expect("tempdir");
    let missing_home = temp.path().join("missing runtime");
    Command::cargo_bin("moon")
        .expect("binary")
        .args([
            "--home",
            missing_home.to_str().expect("utf8"),
            "--json",
            "update",
            "--check",
            "--dry-run",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(r#""code":"invalid_arguments""#));
    assert!(!missing_home.exists());
}
