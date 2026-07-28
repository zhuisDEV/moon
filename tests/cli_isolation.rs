use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn init_only_writes_to_explicit_test_home() {
    let temp = tempfile::tempdir().expect("tempdir");
    let test_home = temp.path().join("moon");
    Command::cargo_bin("moon")
        .expect("binary")
        .args([
            "--home",
            test_home.to_str().expect("utf8"),
            "--dimensions",
            "64",
            "--json",
            "init",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("moon.sqlite"));
    assert!(test_home.join("state/moon.sqlite").is_file());
}

#[test]
fn commands_never_offer_cutover_or_delete_operations() {
    Command::cargo_bin("moon")
        .expect("binary")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("import-legacy"))
        .stdout(predicate::str::contains("record"))
        .stdout(predicate::str::contains("distill"))
        .stdout(predicate::str::contains("context"))
        .stdout(predicate::str::contains("cutover").not())
        .stdout(predicate::str::contains("uninstall").not());
}

#[test]
fn health_never_creates_a_missing_runtime() {
    let temp = tempfile::tempdir().expect("tempdir");
    let missing_home = temp.path().join("misspelled-runtime");
    Command::cargo_bin("moon")
        .expect("binary")
        .args([
            "--home",
            missing_home.to_str().expect("utf8"),
            "--dimensions",
            "64",
            "--json",
            "health",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(r#""code":"operation_failed""#))
        .stderr(predicate::str::contains("does not exist"));
    assert!(!missing_home.exists());
}

#[test]
fn json_mode_returns_structured_argument_and_operation_errors() {
    Command::cargo_bin("moon")
        .expect("binary")
        .args(["--json", "unknown-command"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(r#""code":"invalid_arguments""#));

    let temp = tempfile::tempdir().expect("tempdir");
    Command::cargo_bin("moon")
        .expect("binary")
        .args([
            "--home",
            temp.path().join("test").to_str().expect("utf8"),
            "--dimensions",
            "64",
            "--json",
            "remember",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(r#""code":"operation_failed""#));
}

#[test]
fn evidence_to_context_cli_workflow_is_self_contained() {
    let temp = tempfile::tempdir().expect("tempdir");
    let test_home = temp.path().join("moon");
    let transcript = temp.path().join("session.txt");
    std::fs::write(
        &transcript,
        "Decision: Moon context packets cite immutable session evidence.",
    )
    .expect("write transcript");

    Command::cargo_bin("moon")
        .expect("binary")
        .args([
            "--home",
            test_home.to_str().expect("utf8"),
            "--dimensions",
            "64",
            "--json",
            "record",
            "--session-id",
            "cli-session",
            "--scope",
            "moon",
            "--file",
            transcript.to_str().expect("utf8"),
            "--completed-at-ms",
            "100",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""changed":true"#));

    Command::cargo_bin("moon")
        .expect("binary")
        .args([
            "--home",
            test_home.to_str().expect("utf8"),
            "--dimensions",
            "64",
            "--json",
            "distill",
            "--key",
            "moon:context-packets",
            "--session-id",
            "cli-session",
            "--scope",
            "moon",
            "--content",
            "Moon context packets cite immutable session evidence.",
            "--evidence-quote",
            "Moon context packets cite immutable session evidence.",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""action":"created"#));

    Command::cargo_bin("moon")
        .expect("binary")
        .args([
            "--home",
            test_home.to_str().expect("utf8"),
            "--dimensions",
            "64",
            "context",
            "--query",
            "immutable session evidence",
            "--scope",
            "moon",
            "--mode",
            "lexical",
            "--max-chars",
            "2000",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("# Moon Context"))
        .stdout(predicate::str::contains("cli-session"))
        .stdout(predicate::str::contains("immutable session evidence"));
}

#[test]
fn evidence_and_distillation_accept_private_payloads_on_stdin() {
    let temp = tempfile::tempdir().expect("tempdir");
    let test_home = temp.path().join("moon");
    let binary = || Command::cargo_bin("moon").expect("binary");

    binary()
        .args([
            "--home",
            test_home.to_str().expect("utf8"),
            "--dimensions",
            "64",
            "--json",
            "record",
            "--session-id",
            "stdin-session",
            "--completed-at-ms",
            "100",
        ])
        .write_stdin("User prefers concise answers.")
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""changed":true"#));

    binary()
        .args([
            "--home",
            test_home.to_str().expect("utf8"),
            "--dimensions",
            "64",
            "--json",
            "distill",
            "--key",
            "user:preference:concise",
            "--session-id",
            "stdin-session",
            "--kind",
            "preference",
            "--proposal-json",
        ])
        .write_stdin(
            r#"{"content":"User prefers concise answers.","evidence_quote":"User prefers concise answers.","title":"Response style"}"#,
        )
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""action":"created"#));
}

#[test]
fn text_context_output_is_empty_when_nothing_is_relevant() {
    let temp = tempfile::tempdir().expect("tempdir");
    let test_home = temp.path().join("moon");
    Command::cargo_bin("moon")
        .expect("binary")
        .args([
            "--home",
            test_home.to_str().expect("utf8"),
            "--dimensions",
            "64",
            "context",
            "--query",
            "Hi lilac",
            "--mode",
            "lexical",
        ])
        .assert()
        .success()
        .stdout(predicate::str::is_empty());
}
