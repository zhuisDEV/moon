use assert_cmd::Command;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::{Arc, Barrier};
use std::time::Duration;

const SCOPE: &str = "topic-cli";
const QUOTE: &str = "Canary SQLite retains source history after withdrawal.";

fn moon(home: &Path) -> Command {
    let mut command = Command::new(assert_cmd::cargo::cargo_bin!("moon"));
    command
        .env_remove("MOON_HOME")
        .env_remove("MOON_DATABASE")
        .env_remove("MOON_EMBEDDING_DIMENSIONS")
        .arg("--home")
        .arg(home)
        .arg("--database")
        .arg(home.join("state/moon.sqlite"))
        .args(["--dimensions", "64"])
        .timeout(Duration::from_secs(20));
    command
}
fn output(command: &mut Command) -> Value {
    serde_json::from_slice(&command.assert().success().get_output().stdout).unwrap()
}
fn text<'a>(value: &'a Value, field: &str) -> &'a str {
    value[field].as_str().unwrap()
}
fn source_claim(home: &Path) -> String {
    output(moon(home).args(["source","import","--scope",SCOPE,"--allow-learning"]).write_stdin(json!({"schema_version":1,"source_key":"article","kind":"external_document","content":QUOTE}).to_string()));
    let prepared = output(moon(home).args([
        "source", "learning", "prepare", "--scope", SCOPE, "--key", "article",
    ]));
    let review = output(
        moon(home)
            .args([
                "source",
                "learning",
                "submit",
                "--scope",
                SCOPE,
                "--job-id",
                text(&prepared, "job_id"),
                "--lease-token",
                text(&prepared, "lease_token"),
            ])
            .write_stdin(json!({"schema_version":1,"claims":[{"quote":QUOTE}]}).to_string()),
    );
    let accepted = output(moon(home).args([
        "source",
        "learning",
        "accept",
        "--scope",
        SCOPE,
        "--job-id",
        text(&prepared, "job_id"),
        "--expected-result-hash",
        text(&review, "result_hash"),
        "--yes",
    ]));
    accepted["claim_ids"][0].as_str().unwrap().to_owned()
}
fn put(home: &Path, claim: &str) -> Value {
    output(moon(home).args(["topic","put","--scope",SCOPE]).write_stdin(json!({"schema_version":1,"key":"retention","title":"Canary SQLite retention","aliases":["Moon 知识库"],"claim_ids":[claim]}).to_string()))
}
fn prepare(home: &Path) -> Value {
    output(moon(home).args(["topic", "prepare", "--scope", SCOPE, "--key", "retention"]))
}
fn submit(home: &Path, prepared: &Value, claim: &str) -> Value {
    output(moon(home).args(["topic","submit","--scope",SCOPE,"--job-id",text(prepared,"job_id"),"--lease-token",text(prepared,"lease_token")]).write_stdin(json!({"schema_version":1,"review_complete":true,"statements":[{"kind":"fact","text":QUOTE,"input_ids":[claim]}]}).to_string()))
}
fn accept(home: &Path, prepared: &Value, review: &Value) -> Value {
    output(moon(home).args([
        "topic",
        "accept",
        "--scope",
        SCOPE,
        "--job-id",
        text(prepared, "job_id"),
        "--expected-result-hash",
        text(review, "result_hash"),
        "--yes",
    ]))
}

#[test]
fn topic_cli_requires_explicit_review_and_exposes_typed_fresh_revisions_as_json() {
    let temp = tempfile::tempdir().unwrap();
    let claim = source_claim(temp.path());
    let created = put(temp.path(), &claim);
    let preview = output(moon(temp.path()).args([
        "topic",
        "prepare",
        "--scope",
        SCOPE,
        "--key",
        "retention",
        "--preview",
    ]));
    assert_eq!(preview["status"], "preview");
    assert!(preview.get("lease_token").is_none());
    let prepared = prepare(temp.path());
    assert_eq!(prepared["automatic_acceptance"], false);
    let review = submit(temp.path(), &prepared, &claim);
    moon(temp.path())
        .args([
            "topic",
            "accept",
            "--scope",
            SCOPE,
            "--job-id",
            text(&prepared, "job_id"),
            "--expected-result-hash",
            text(&review, "result_hash"),
        ])
        .assert()
        .failure();
    let get =
        output(moon(temp.path()).args(["topic", "get", "--scope", SCOPE, "--key", "retention"]));
    assert_eq!(get["freshness"], "uncompiled");
    let accepted = accept(temp.path(), &prepared, &review);
    assert_eq!(accept(temp.path(), &prepared, &review), accepted);
    let get =
        output(moon(temp.path()).args(["topic", "get", "--scope", SCOPE, "--key", "moon 知识库"]));
    assert_eq!(get["topic_id"], created["topic_id"]);
    assert_eq!(get["revision_id"], accepted["revision_id"]);
    assert_eq!(get["freshness"], "fresh");
    assert_eq!(get["statements"][0]["kind"], "fact");
    assert_eq!(get["statements"][0]["text"], QUOTE);
    assert_eq!(get["statements"][0]["citations"][0]["quote"], QUOTE);
    let history = output(moon(temp.path()).args([
        "topic",
        "history",
        "--scope",
        SCOPE,
        "--key",
        "retention",
    ]));
    assert_eq!(history["revisions"].as_array().unwrap().len(), 1);
    let list = output(moon(temp.path()).args(["topic", "list", "--scope", SCOPE]));
    assert_eq!(list["topics"].as_array().unwrap().len(), 1);
    assert!(!temp.path().join("memory").exists());
}

#[test]
fn definition_updates_require_the_current_generation_and_invalidate_old_heads() {
    let temp = tempfile::tempdir().unwrap();
    let claim = source_claim(temp.path());
    let created = put(temp.path(), &claim);
    let prepared = prepare(temp.path());
    let review = submit(temp.path(), &prepared, &claim);
    accept(temp.path(), &prepared, &review);
    let changed=json!({"schema_version":1,"key":"retention","title":"Updated retention explanation","claim_ids":[claim]}).to_string();
    moon(temp.path())
        .args(["topic", "put", "--scope", SCOPE])
        .write_stdin(changed.clone())
        .assert()
        .failure();
    let generation = created["generation"].as_i64().unwrap().to_string();
    let updated = output(
        moon(temp.path())
            .args([
                "topic",
                "put",
                "--scope",
                SCOPE,
                "--expected-generation",
                &generation,
            ])
            .write_stdin(changed),
    );
    assert_eq!(
        updated["generation"],
        created["generation"].as_i64().unwrap() + 1
    );
    moon(temp.path()).args(["topic","put","--scope",SCOPE,"--expected-generation",&generation]).write_stdin(json!({"schema_version":1,"key":"retention","title":"Stale conflicting update","claim_ids":[claim]}).to_string()).assert().failure();
    assert_ne!(
        output(moon(temp.path()).args(["topic", "get", "--scope", SCOPE, "--key", "retention"]))["freshness"],
        "fresh"
    );
}

#[test]
fn readonly_topic_commands_never_create_a_home_or_migrate_an_older_database() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("missing");
    let commands = vec![
        vec!["topic", "get", "--scope", SCOPE, "--key", "retention"],
        vec!["topic", "list", "--scope", SCOPE],
        vec!["topic", "history", "--scope", SCOPE, "--key", "retention"],
        vec!["topic", "status", "--scope", SCOPE],
        vec!["topic", "prepare", "--scope", SCOPE, "--preview"],
    ];
    for args in &commands {
        moon(&missing).args(args).assert().failure();
        assert!(!missing.exists());
    }
    let older = temp.path().join("old");
    std::fs::create_dir_all(older.join("state")).unwrap();
    let database = older.join("state/moon.sqlite");
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection.execute_batch("PRAGMA user_version=10;").unwrap();
    drop(connection);
    let before = std::fs::read(&database).unwrap();
    for args in &commands {
        moon(&older).args(args).assert().failure();
    }
    assert_eq!(std::fs::read(&database).unwrap(), before);
    assert_eq!(
        rusqlite::Connection::open(database)
            .unwrap()
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        10
    );
}

#[test]
fn strict_topic_json_errors_never_echo_untrusted_fields_or_create_storage() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("missing");
    let sensitive = "never-echo-this-private-fixture-value";
    let fake = "scl_00000000000000000000000000000000";
    for body in [
        format!("{{broken-json-{sensitive}"),
        json!({"schema_version":1,"key":"x","title":"Title","claim_ids":[fake],"unexpected":sensitive}).to_string(),
        json!({"schema_version":2,"key":"x","title":sensitive,"claim_ids":[fake]}).to_string(),
        json!({"schema_version":1,"key":"x","title":sensitive,"claim_ids":[]}).to_string(),
    ] {
        let assertion=moon(&missing).args(["topic","put","--scope",SCOPE]).write_stdin(body).assert().failure();
        let output=assertion.get_output(); let combined=format!("{}{}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
        assert!(!combined.contains(sensitive)); assert!(!missing.exists());
    }
    let body=json!({"schema_version":1,"review_complete":true,"statements":[{"kind":"question","text":sensitive,"input_ids":[]}],"accept":true}).to_string();
    let assertion = moon(&missing)
        .args([
            "topic",
            "submit",
            "--scope",
            SCOPE,
            "--job-id",
            "tcj_00000000000000000000000000000000",
            "--lease-token",
            "tct_00000000000000000000000000000000",
        ])
        .write_stdin(body)
        .assert()
        .failure();
    assert!(!String::from_utf8_lossy(&assertion.get_output().stdout).contains(sensitive));
    assert!(!String::from_utf8_lossy(&assertion.get_output().stderr).contains(sensitive));
    assert!(!missing.exists());
}

#[test]
fn topic_file_intake_rejects_symlinks_nonregular_inputs_and_oversized_payloads() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("missing");
    let oversized = "x".repeat(8 * 1024 * 1024 + 1);
    moon(&missing)
        .args(["topic", "put", "--scope", SCOPE])
        .write_stdin(oversized)
        .assert()
        .failure();
    assert!(!missing.exists());
    moon(&missing)
        .args(["topic", "put", "--scope", SCOPE, "--input"])
        .arg(temp.path())
        .assert()
        .failure();
    assert!(!missing.exists());
    #[cfg(unix)]
    {
        let target = temp.path().join("input.json");
        std::fs::write(&target, b"{}").unwrap();
        let link = temp.path().join("link.json");
        std::os::unix::fs::symlink(target, &link).unwrap();
        moon(&missing)
            .args(["topic", "put", "--scope", SCOPE, "--input"])
            .arg(link)
            .assert()
            .failure();
        assert!(!missing.exists());
    }
}

#[test]
fn separate_cli_processes_cannot_obtain_two_leases_for_one_topic_job() {
    let temp = tempfile::tempdir().unwrap();
    let claim = source_claim(temp.path());
    put(temp.path(), &claim);
    let barrier = Arc::new(Barrier::new(2));
    let handles = (0..2)
        .map(|_| {
            let home = temp.path().to_path_buf();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                prepare(&home)
            })
        })
        .collect::<Vec<_>>();
    let mut statuses = handles
        .into_iter()
        .map(|h| text(&h.join().unwrap(), "status").to_owned())
        .collect::<Vec<_>>();
    statuses.sort();
    assert_eq!(statuses, vec!["busy", "prepared"]);
    let status = output(moon(temp.path()).args(["topic", "status", "--scope", SCOPE]));
    assert_eq!(status["jobs"][0]["attempts"], 1);
}

#[test]
fn source_withdrawal_blocks_pending_topic_approval_and_does_not_cross_scopes() {
    let temp = tempfile::tempdir().unwrap();
    let claim = source_claim(temp.path());
    put(temp.path(), &claim);
    let prepared = prepare(temp.path());
    let review = submit(temp.path(), &prepared, &claim);
    moon(temp.path())
        .args([
            "topic",
            "accept",
            "--scope",
            "other",
            "--job-id",
            text(&prepared, "job_id"),
            "--expected-result-hash",
            text(&review, "result_hash"),
            "--yes",
        ])
        .assert()
        .failure();
    let state =
        output(moon(temp.path()).args(["source", "state", "--scope", SCOPE, "--key", "article"]));
    output(moon(temp.path()).args([
        "source",
        "withdraw",
        "--scope",
        SCOPE,
        "--key",
        "article",
        "--expected-revision",
        text(&state, "revision_id"),
        "--expected-generation",
        &state["generation"].as_i64().unwrap().to_string(),
        "--reason",
        "Withdraw fixture support",
    ]));
    moon(temp.path())
        .args([
            "topic",
            "accept",
            "--scope",
            SCOPE,
            "--job-id",
            text(&prepared, "job_id"),
            "--expected-result-hash",
            text(&review, "result_hash"),
            "--yes",
        ])
        .assert()
        .failure();
    let list = output(moon(temp.path()).args(["topic", "list", "--scope", "other"]));
    assert!(list["topics"].as_array().unwrap().is_empty());
}

#[test]
fn topic_retry_and_rejection_use_fixed_failure_codes_and_remain_unindexed() {
    let temp = tempfile::tempdir().unwrap();
    let claim = source_claim(temp.path());
    put(temp.path(), &claim);
    let first = prepare(temp.path());
    moon(temp.path())
        .args([
            "topic",
            "fail",
            "--scope",
            SCOPE,
            "--job-id",
            text(&first, "job_id"),
            "--lease-token",
            text(&first, "lease_token"),
            "--error-code",
            "arbitrary-provider-response",
        ])
        .assert()
        .failure();
    output(moon(temp.path()).args([
        "topic",
        "fail",
        "--scope",
        SCOPE,
        "--job-id",
        text(&first, "job_id"),
        "--lease-token",
        text(&first, "lease_token"),
        "--error-code",
        "interrupted",
    ]));
    output(moon(temp.path()).args([
        "topic",
        "retry",
        "--scope",
        SCOPE,
        "--job-id",
        text(&first, "job_id"),
    ]));
    let second = prepare(temp.path());
    let review = submit(temp.path(), &second, &claim);
    output(moon(temp.path()).args([
        "topic",
        "reject",
        "--scope",
        SCOPE,
        "--job-id",
        text(&second, "job_id"),
        "--expected-result-hash",
        text(&review, "result_hash"),
        "--reason",
        "Insufficiently useful explanation",
    ]));
    let get =
        output(moon(temp.path()).args(["topic", "get", "--scope", SCOPE, "--key", "retention"]));
    assert_eq!(get["freshness"], "uncompiled");
}

#[test]
fn model_route_failure_is_dead_until_explicit_cli_retry() {
    let temp = tempfile::tempdir().unwrap();
    let claim = source_claim(temp.path());
    put(temp.path(), &claim);
    let first = prepare(temp.path());
    output(moon(temp.path()).args([
        "topic",
        "fail",
        "--scope",
        SCOPE,
        "--job-id",
        text(&first, "job_id"),
        "--lease-token",
        text(&first, "lease_token"),
        "--error-code",
        "model_route_mismatch",
    ]));
    let status = output(moon(temp.path()).args([
        "topic",
        "status",
        "--scope",
        SCOPE,
        "--job-id",
        text(&first, "job_id"),
    ]));
    assert_eq!(status["jobs"][0]["status"], "dead");
    assert_eq!(status["jobs"][0]["attempts"], 1);
    assert_eq!(status["jobs"][0]["last_error"], "model_route_mismatch");
    assert_ne!(prepare(temp.path())["status"], "prepared");
    output(moon(temp.path()).args([
        "topic",
        "retry",
        "--scope",
        SCOPE,
        "--job-id",
        text(&first, "job_id"),
    ]));
    let second = prepare(temp.path());
    assert_eq!(second["status"], "prepared");
    assert_ne!(second["lease_token"], first["lease_token"]);
}
