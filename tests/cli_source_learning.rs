use assert_cmd::Command;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::{Arc, Barrier};
use std::time::Duration;

const QUOTE: &str = "Canary uses SQLite for source storage.";

fn moon(home: &Path) -> Command {
    let mut command = Command::new(assert_cmd::cargo::cargo_bin!("moon"));
    command
        .env_remove("MOON_HOME")
        .env_remove("MOON_DATABASE")
        .env_remove("MOON_EMBEDDING_DIMENSIONS")
        .arg("--home")
        .arg(home)
        .args(["--dimensions", "64"])
        .timeout(Duration::from_secs(15));
    command
}

fn output(command: &mut Command) -> Value {
    serde_json::from_slice(&command.assert().success().get_output().stdout).unwrap()
}

fn text<'a>(value: &'a Value, field: &str) -> &'a str {
    value[field].as_str().unwrap()
}

fn capture(home: &Path) -> Value {
    output(moon(home).args(["source","import","--scope","canary","--allow-learning"]).write_stdin(json!({
        "schema_version":1,"source_key":"article","kind":"external_document","author":"Example author","content":QUOTE
    }).to_string()))
}

fn prepare(home: &Path) -> Value {
    output(moon(home).args([
        "source", "learning", "prepare", "--scope", "canary", "--key", "article",
    ]))
}

fn submit(home: &Path, prepared: &Value) -> Value {
    output(
        moon(home)
            .args([
                "source",
                "learning",
                "submit",
                "--scope",
                "canary",
                "--job-id",
                text(prepared, "job_id"),
                "--lease-token",
                text(prepared, "lease_token"),
            ])
            .write_stdin(json!({"schema_version":1,"claims":[{"quote":QUOTE}]}).to_string()),
    )
}

fn accept(home: &Path, prepared: &Value, review: &Value) -> Value {
    output(moon(home).args([
        "source",
        "learning",
        "accept",
        "--scope",
        "canary",
        "--job-id",
        text(prepared, "job_id"),
        "--expected-result-hash",
        text(review, "result_hash"),
        "--yes",
    ]))
}

#[test]
fn source_learning_cli_requires_review_approval_and_preserves_original_citations() {
    let temp = tempfile::tempdir().unwrap();
    let source = capture(temp.path());
    let preview = output(moon(temp.path()).args([
        "source",
        "learning",
        "prepare",
        "--scope",
        "canary",
        "--key",
        "article",
        "--preview",
    ]));
    assert_eq!(preview["status"], "preview");
    assert!(preview.get("lease_token").is_none());
    let prepared = prepare(temp.path());
    assert_eq!(prepared["source"]["revision_id"], source["revision_id"]);
    let review = submit(temp.path(), &prepared);
    let list = output(moon(temp.path()).args(["source", "claims", "list", "--scope", "canary"]));
    assert!(list["claims"].as_array().unwrap().is_empty());
    moon(temp.path())
        .args([
            "source",
            "learning",
            "accept",
            "--scope",
            "canary",
            "--job-id",
            text(&prepared, "job_id"),
            "--expected-result-hash",
            text(&review, "result_hash"),
        ])
        .assert()
        .failure();
    let accepted = accept(temp.path(), &prepared, &review);
    assert_eq!(accepted["status"], "complete");
    let claim = output(moon(temp.path()).args([
        "source",
        "claims",
        "get",
        "--scope",
        "canary",
        "--claim-id",
        accepted["claim_ids"][0].as_str().unwrap(),
    ]));
    assert_eq!(claim["content"], QUOTE);
    assert_eq!(claim["kind"], "author_statement");
    assert_eq!(claim["active"], true);
    assert!(!claim["citations"].as_array().unwrap().is_empty());
    assert_eq!(accept(temp.path(), &prepared, &review), accepted);
    let status = output(moon(temp.path()).args([
        "source",
        "learning",
        "status",
        "--scope",
        "canary",
        "--job-id",
        text(&prepared, "job_id"),
    ]));
    assert_eq!(status["jobs"][0]["status"], "complete");
}

#[test]
fn lifecycle_cli_rejects_stale_generation_and_hides_withdrawn_claims() {
    let temp = tempfile::tempdir().unwrap();
    capture(temp.path());
    let prepared = prepare(temp.path());
    let review = submit(temp.path(), &prepared);
    accept(temp.path(), &prepared, &review);
    let state = output(
        moon(temp.path()).args(["source", "state", "--scope", "canary", "--key", "article"]),
    );
    let generation = state["generation"].as_i64().unwrap().to_string();
    output(moon(temp.path()).args([
        "source",
        "withdraw",
        "--scope",
        "canary",
        "--key",
        "article",
        "--expected-revision",
        text(&state, "revision_id"),
        "--expected-generation",
        &generation,
        "--reason",
        "Reference withdrawn",
    ]));
    let claims = output(moon(temp.path()).args(["source", "claims", "list", "--scope", "canary"]));
    assert!(claims["claims"].as_array().unwrap().is_empty());
    moon(temp.path())
        .args([
            "source",
            "restore",
            "--scope",
            "canary",
            "--key",
            "article",
            "--expected-revision",
            text(&state, "revision_id"),
            "--expected-generation",
            &generation,
            "--reason",
            "Stale request",
        ])
        .assert()
        .failure();
    let current = output(
        moon(temp.path()).args(["source", "state", "--scope", "canary", "--key", "article"]),
    );
    let generation = current["generation"].as_i64().unwrap().to_string();
    output(moon(temp.path()).args([
        "source",
        "restore",
        "--scope",
        "canary",
        "--key",
        "article",
        "--expected-revision",
        text(&current, "revision_id"),
        "--expected-generation",
        &generation,
        "--reason",
        "Reference restored",
    ]));
    let claims = output(moon(temp.path()).args(["source", "claims", "list", "--scope", "canary"]));
    assert_eq!(claims["claims"].as_array().unwrap().len(), 1);
}

#[test]
fn destructive_cli_needs_yes_and_a_current_dependency_plan() {
    let temp = tempfile::tempdir().unwrap();
    capture(temp.path());
    let before = output(moon(temp.path()).args([
        "source",
        "delete-preview",
        "--scope",
        "canary",
        "--key",
        "article",
    ]));
    moon(temp.path())
        .args([
            "source",
            "delete-apply",
            "--scope",
            "canary",
            "--key",
            "article",
            "--expected-plan-hash",
            text(&before, "plan_hash"),
        ])
        .assert()
        .failure();
    let prepared = prepare(temp.path());
    let review = submit(temp.path(), &prepared);
    accept(temp.path(), &prepared, &review);
    moon(temp.path())
        .args([
            "source",
            "delete-apply",
            "--scope",
            "canary",
            "--key",
            "article",
            "--expected-plan-hash",
            text(&before, "plan_hash"),
            "--yes",
        ])
        .assert()
        .failure();
    let current = output(moon(temp.path()).args([
        "source",
        "delete-preview",
        "--scope",
        "canary",
        "--key",
        "article",
    ]));
    output(moon(temp.path()).args([
        "source",
        "delete-apply",
        "--scope",
        "canary",
        "--key",
        "article",
        "--expected-plan-hash",
        text(&current, "plan_hash"),
        "--yes",
    ]));
    let state = output(
        moon(temp.path()).args(["source", "state", "--scope", "canary", "--key", "article"]),
    );
    assert_eq!(state["status"], "deleted");
    moon(temp.path())
        .args(["source", "get", "--scope", "canary", "--key", "article"])
        .assert()
        .failure();
    let claims = output(moon(temp.path()).args([
        "source",
        "claims",
        "list",
        "--scope",
        "canary",
        "--include-inactive",
    ]));
    assert!(claims["claims"].as_array().unwrap().is_empty());
}

#[test]
fn read_only_learning_lifecycle_claim_commands_do_not_create_or_migrate_storage() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("missing");
    for args in [
        vec!["source", "learning", "status", "--scope", "canary"],
        vec![
            "source",
            "learning",
            "prepare",
            "--scope",
            "canary",
            "--preview",
        ],
        vec!["source", "state", "--scope", "canary", "--key", "article"],
        vec![
            "source",
            "delete-preview",
            "--scope",
            "canary",
            "--key",
            "article",
        ],
        vec!["source", "claims", "list", "--scope", "canary"],
        vec![
            "source",
            "claims",
            "get",
            "--scope",
            "canary",
            "--claim-id",
            "scl_00000000000000000000000000000000",
        ],
    ] {
        moon(&missing).args(args).assert().failure();
        assert!(!missing.exists());
    }
    let state = temp.path().join("old/state");
    std::fs::create_dir_all(&state).unwrap();
    let connection = rusqlite::Connection::open(state.join("moon.sqlite")).unwrap();
    connection.execute_batch("PRAGMA user_version=9;").unwrap();
    moon(&temp.path().join("old"))
        .args([
            "source",
            "learning",
            "prepare",
            "--scope",
            "canary",
            "--preview",
        ])
        .assert()
        .failure();
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        9
    );
}

#[test]
fn strict_submit_schema_and_failure_codes_do_not_echo_or_store_remote_error_text() {
    let temp = tempfile::tempdir().unwrap();
    capture(temp.path());
    let prepared = prepare(temp.path());
    for bad in [
        json!({"schema_version":1,"claims":[{"quote":QUOTE,"synthetic-secret-field":"synthetic-secret-value"}]}).to_string(),
        "{synthetic-secret-malformed-json}".into(),
        json!({"schema_version":999,"claims":[{"quote":"synthetic-secret-model-answer"}]}).to_string(),
    ] {
        let result=moon(temp.path()).args(["source","learning","submit","--scope","canary","--job-id",text(&prepared,"job_id"),"--lease-token",text(&prepared,"lease_token")]).write_stdin(bad).assert().failure();
        assert!(!String::from_utf8_lossy(&result.get_output().stderr).contains("synthetic-secret"));
        assert!(result.get_output().stdout.is_empty());
    }
    let failed = moon(temp.path())
        .args([
            "source",
            "learning",
            "fail",
            "--scope",
            "canary",
            "--job-id",
            text(&prepared, "job_id"),
            "--lease-token",
            text(&prepared, "lease_token"),
            "--error-code",
            "synthetic-secret-provider-body",
        ])
        .assert()
        .failure();
    assert!(
        !String::from_utf8_lossy(&failed.get_output().stderr)
            .contains("synthetic-secret-provider-body")
    );
    let status = output(moon(temp.path()).args([
        "source",
        "learning",
        "status",
        "--scope",
        "canary",
        "--job-id",
        text(&prepared, "job_id"),
    ]));
    assert_eq!(status["jobs"][0]["status"], "leased");
    let review = submit(temp.path(), &prepared);
    accept(temp.path(), &prepared, &review);
}

#[test]
fn failed_work_can_retry_but_rejected_review_stays_unindexed() {
    let temp = tempfile::tempdir().unwrap();
    capture(temp.path());
    let first = prepare(temp.path());
    output(moon(temp.path()).args([
        "source",
        "learning",
        "fail",
        "--scope",
        "canary",
        "--job-id",
        text(&first, "job_id"),
        "--lease-token",
        text(&first, "lease_token"),
        "--error-code",
        "interrupted",
    ]));
    output(moon(temp.path()).args([
        "source",
        "learning",
        "retry",
        "--scope",
        "canary",
        "--job-id",
        text(&first, "job_id"),
    ]));
    let second = prepare(temp.path());
    assert_ne!(second["lease_token"], first["lease_token"]);
    let review = submit(temp.path(), &second);
    let rejected = output(moon(temp.path()).args([
        "source",
        "learning",
        "reject",
        "--scope",
        "canary",
        "--job-id",
        text(&second, "job_id"),
        "--expected-result-hash",
        text(&review, "result_hash"),
        "--reason",
        "Not useful for this project",
    ]));
    assert_eq!(rejected["decision"], "rejected");
    moon(temp.path())
        .args([
            "source",
            "learning",
            "accept",
            "--scope",
            "canary",
            "--job-id",
            text(&second, "job_id"),
            "--expected-result-hash",
            text(&review, "result_hash"),
            "--yes",
        ])
        .assert()
        .failure();
    let claims = output(moon(temp.path()).args(["source", "claims", "list", "--scope", "canary"]));
    assert!(claims["claims"].as_array().unwrap().is_empty());
}

#[test]
fn configuration_failure_is_dead_until_explicit_cli_retry() {
    let temp = tempfile::tempdir().unwrap();
    capture(temp.path());
    let first = prepare(temp.path());
    output(moon(temp.path()).args([
        "source",
        "learning",
        "fail",
        "--scope",
        "canary",
        "--job-id",
        text(&first, "job_id"),
        "--lease-token",
        text(&first, "lease_token"),
        "--error-code",
        "authentication_failed",
    ]));
    let status = output(moon(temp.path()).args([
        "source",
        "learning",
        "status",
        "--scope",
        "canary",
        "--job-id",
        text(&first, "job_id"),
    ]));
    assert_eq!(status["jobs"][0]["status"], "dead");
    assert_eq!(status["jobs"][0]["attempts"], 1);
    assert_eq!(status["jobs"][0]["last_error"], "authentication_failed");
    assert_ne!(prepare(temp.path())["status"], "prepared");
    output(moon(temp.path()).args([
        "source",
        "learning",
        "retry",
        "--scope",
        "canary",
        "--job-id",
        text(&first, "job_id"),
    ]));
    let second = prepare(temp.path());
    assert_eq!(second["status"], "prepared");
    assert_ne!(second["lease_token"], first["lease_token"]);
}

#[test]
fn separate_cli_processes_cannot_both_lease_the_same_source_job() {
    let temp = tempfile::tempdir().unwrap();
    capture(temp.path());
    let barrier = Arc::new(Barrier::new(2));
    let workers = [0, 1].map(|_| {
        let home = temp.path().to_path_buf();
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || {
            // Each worker starts a separate moon process with its own SQLite
            // connection; the barrier aligns process launch, not Rust calls.
            barrier.wait();
            prepare(&home)
        })
    });
    let results = workers.map(|worker| worker.join().unwrap());
    assert_eq!(
        results
            .iter()
            .filter(|result| result["status"] == "prepared")
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| result["status"] == "busy")
            .count(),
        1
    );
    let winner = results
        .iter()
        .find(|result| result["status"] == "prepared")
        .unwrap();
    let status = output(moon(temp.path()).args([
        "source",
        "learning",
        "status",
        "--scope",
        "canary",
        "--job-id",
        text(winner, "job_id"),
    ]));
    assert_eq!(status["jobs"].as_array().unwrap().len(), 1);
    assert_eq!(status["jobs"][0]["status"], "leased");
    assert_eq!(status["jobs"][0]["attempts"], 1);
}
