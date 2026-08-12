use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;

fn moon(home: &std::path::Path) -> Command {
    let mut command = Command::new(assert_cmd::cargo::cargo_bin!("moon"));
    command.args([
        "--home",
        home.to_str().expect("utf8 home"),
        "--dimensions",
        "64",
        "--json",
    ]);
    command
}

#[test]
fn metrics_cli_observes_reviews_exports_and_prunes_without_content() {
    let temp = tempfile::tempdir().expect("tempdir");
    let home = temp.path().join("moon");

    moon(&home)
        .args([
            "context",
            "--query",
            "private canary phrase that must never enter metrics",
            "--mode",
            "lexical",
        ])
        .assert()
        .success();

    let recent_output = moon(&home)
        .args(["metrics", "recent", "--since", "1h"])
        .output()
        .expect("recent metrics");
    assert!(recent_output.status.success(), "{recent_output:?}");
    let recent: Value = serde_json::from_slice(&recent_output.stdout).expect("recent JSON");
    let request_id = recent[0]["request_id"]
        .as_str()
        .expect("request id")
        .to_string();
    let recent_json = String::from_utf8(recent_output.stdout).expect("utf8 JSON");
    assert!(!recent_json.contains("private canary phrase"));

    moon(&home)
        .args([
            "metrics",
            "review",
            "--request",
            &request_id,
            "--outcome",
            "correct-empty",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            r#""review_outcome":"correct_empty""#,
        ));

    moon(&home)
        .args([
            "metrics",
            "record-runtime",
            "--kind",
            "learning",
            "--status",
            "ok",
            "--duration-us",
            "10",
            "--evidence-changed",
            "--learning-eligible",
            "--proposed-memories",
            "1",
            "--accepted-memories",
            "1",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""event_id""#));

    moon(&home)
        .args(["metrics", "summary", "--since", "1h"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""context_requests":1"#))
        .stdout(predicate::str::contains(r#""correct_empty":1"#))
        .stdout(predicate::str::contains(r#""learning_events":1"#))
        .stdout(predicate::str::contains(r#""accepted_memories":1"#));

    let export = temp.path().join("metrics.json");
    moon(&home)
        .args([
            "metrics",
            "export",
            "--since",
            "1h",
            "--destination",
            export.to_str().expect("utf8 export"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""redacted":true"#));
    let exported = std::fs::read_to_string(export).expect("read export");
    assert!(!exported.contains("private canary phrase"));

    moon(&home)
        .args(["metrics", "prune", "--older-than", "1s"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""dry_run":true"#));
}
