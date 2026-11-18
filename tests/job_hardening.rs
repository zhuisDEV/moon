use moon::Store;
use moon::source_learning::{SourcePrepareRequest, SourceProposalInput};
use moon::topics::{TopicPrepareRequest, TopicProposalInput};
use rusqlite::{Connection, types::Value as SqlValue};
use serde_json::{Value, json};

const SCOPE: &str = "hardening-canary";
const QUOTE: &str = "The rehearsal needs operator approval before restart.";

fn text<'a>(value: &'a Value, name: &str) -> &'a str {
    value[name].as_str().unwrap()
}

fn source_request(key: &str) -> SourcePrepareRequest {
    SourcePrepareRequest {
        scope: SCOPE.into(),
        key: Some(key.into()),
        max_bytes: 65536,
        lease_ms: 60000,
        preview: false,
    }
}

fn topic_request() -> TopicPrepareRequest {
    TopicPrepareRequest {
        scope: SCOPE.into(),
        key: Some("pending-topic".into()),
        max_bytes: 65536,
        lease_ms: 60000,
        preview: false,
    }
}

fn fixture() -> (tempfile::TempDir, Store, Value, Value) {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::open(temp.path().join("state/moon.sqlite"), 64).unwrap();
    for key in ["foundation", "pending-source", "queued-source"] {
        store.source_import(SCOPE, serde_json::from_value(json!({"schema_version":1,"source_key":key,"kind":"external_document","content":QUOTE})).unwrap(), None, true).unwrap();
    }
    let prepared = store
        .source_learning_prepare(&source_request("foundation"))
        .unwrap();
    let input: SourceProposalInput =
        serde_json::from_value(json!({"schema_version":1,"claims":[{"quote":QUOTE}]})).unwrap();
    let reviewed = store
        .source_learning_submit(
            SCOPE,
            text(&prepared, "job_id"),
            text(&prepared, "lease_token"),
            input,
        )
        .unwrap();
    let accepted = store
        .source_learning_accept(
            SCOPE,
            text(&prepared, "job_id"),
            text(&reviewed, "result_hash"),
        )
        .unwrap();
    let claim = accepted["claim_ids"][0].as_str().unwrap();
    for key in ["published-topic", "pending-topic"] {
        store.topic_put(SCOPE, serde_json::from_value(json!({"schema_version":1,"key":key,"title":"Rehearsal restart approval","claim_ids":[claim],"topic_ids":[]})).unwrap(), None).unwrap();
    }
    let mut request = topic_request();
    request.key = Some("published-topic".into());
    let prepared = store.topic_prepare(&request).unwrap();
    let input: TopicProposalInput = serde_json::from_value(json!({"schema_version":1,"review_complete":true,"statements":[{"kind":"fact","text":QUOTE,"input_ids":[claim]}]})).unwrap();
    let reviewed = store
        .topic_submit(
            SCOPE,
            text(&prepared, "job_id"),
            text(&prepared, "lease_token"),
            input,
        )
        .unwrap();
    store
        .topic_accept(
            SCOPE,
            text(&prepared, "job_id"),
            text(&reviewed, "result_hash"),
        )
        .unwrap();
    let source = store
        .source_learning_prepare(&source_request("pending-source"))
        .unwrap();
    let topic = store.topic_prepare(&topic_request()).unwrap();
    (temp, store, source, topic)
}

fn statuses(store: &Store, source: &Value, topic: &Value) -> [Value; 2] {
    [
        store
            .source_learning_status(SCOPE, Some(text(source, "job_id")), 10)
            .unwrap()["jobs"][0]
            .clone(),
        store
            .topic_status(SCOPE, Some(text(topic, "job_id")), 10)
            .unwrap()["jobs"][0]
            .clone(),
    ]
}

fn fail_both(store: &mut Store, source: &Value, topic: &Value, code: &str) {
    store
        .source_learning_fail(
            SCOPE,
            text(source, "job_id"),
            text(source, "lease_token"),
            code,
        )
        .unwrap();
    store
        .topic_fail(
            SCOPE,
            text(topic, "job_id"),
            text(topic, "lease_token"),
            code,
        )
        .unwrap();
}

#[test]
fn configuration_failures_stop_early_survive_restart_and_require_explicit_retry() {
    for code in [
        "runtime_unavailable",
        "runtime_setup_failed",
        "authentication_failed",
        "model_unavailable",
        "model_route_mismatch",
    ] {
        let (temp, mut store, source, topic) = fixture();
        fail_both(&mut store, &source, &topic, code);
        drop(store);
        let mut store = Store::open(temp.path().join("state/moon.sqlite"), 64).unwrap();
        for job in statuses(&store, &source, &topic) {
            assert_eq!(job["status"], "dead");
            assert_eq!(job["attempts"], 1);
            assert_eq!(job["last_error"], code);
            assert_eq!(job["not_before_ms"], 0);
        }
        assert_ne!(
            store
                .source_learning_prepare(&source_request("pending-source"))
                .unwrap()["status"],
            "prepared"
        );
        assert_ne!(
            store.topic_prepare(&topic_request()).unwrap()["status"],
            "prepared"
        );
        store
            .source_learning_retry(SCOPE, text(&source, "job_id"))
            .unwrap();
        store.topic_retry(SCOPE, text(&topic, "job_id")).unwrap();
        let fresh_source = store
            .source_learning_prepare(&source_request("pending-source"))
            .unwrap();
        let fresh_topic = store.topic_prepare(&topic_request()).unwrap();
        assert_ne!(source["lease_token"], fresh_source["lease_token"]);
        assert_ne!(topic["lease_token"], fresh_topic["lease_token"]);
        assert!(
            store
                .source_learning_fail(
                    SCOPE,
                    text(&source, "job_id"),
                    text(&source, "lease_token"),
                    code
                )
                .is_err()
        );
        assert!(
            store
                .topic_fail(
                    SCOPE,
                    text(&topic, "job_id"),
                    text(&topic, "lease_token"),
                    code
                )
                .is_err()
        );
        for job in statuses(&store, &source, &topic) {
            assert_eq!(job["status"], "leased");
            assert_eq!(job["attempts"], 1);
            assert!(job["last_error"].is_null());
        }
        // A later withdrawal must replace, not inherit, the old setup diagnostic.
        fail_both(&mut store, &fresh_source, &fresh_topic, code);
        for key in ["pending-source", "foundation"] {
            let state = store.source_state(SCOPE, key).unwrap();
            store
                .source_set_withdrawn(
                    SCOPE,
                    key,
                    text(&state, "revision_id"),
                    state["generation"].as_i64().unwrap(),
                    true,
                    "Fixture withdrawn",
                )
                .unwrap();
        }
        let jobs = statuses(&store, &source, &topic);
        assert_eq!(jobs[0]["last_error"], "withdrawn");
        assert_eq!(jobs[1]["last_error"], "inputs_changed");
        assert!(jobs.iter().all(|j| j["status"] == "cancelled"));
        assert!(store.health().unwrap().ok);
    }
}

#[test]
fn model_timeouts_keep_backoff_and_exhaust_the_normal_attempt_budget() {
    let (_temp, mut store, mut source, mut topic) = fixture();
    for attempt in 1..=3 {
        fail_both(&mut store, &source, &topic, "model_timeout");
        for job in statuses(&store, &source, &topic) {
            assert_eq!(job["status"], if attempt == 3 { "dead" } else { "queued" });
            assert_eq!(job["last_error"], "model_timeout");
            assert_eq!(job["attempts"], attempt);
            assert!(job["not_before_ms"].as_i64().unwrap() > 0);
        }
        if attempt < 3 {
            assert_ne!(
                store
                    .source_learning_prepare(&source_request("pending-source"))
                    .unwrap()["status"],
                "prepared"
            );
            assert_ne!(
                store.topic_prepare(&topic_request()).unwrap()["status"],
                "prepared"
            );
            // Advance the recorded deadline, without sleeping or changing policy.
            Connection::open(store.path()).unwrap().execute_batch("UPDATE source_learning_jobs SET not_before_ms=0; UPDATE topic_jobs SET not_before_ms=0;").unwrap();
            source = store
                .source_learning_prepare(&source_request("pending-source"))
                .unwrap();
            topic = store.topic_prepare(&topic_request()).unwrap();
        }
    }
    assert!(store.health().unwrap().ok);
}

#[test]
fn arbitrary_failure_bodies_and_invalid_diagnostic_states_are_rejected() {
    let (_temp, mut store, source, topic) = fixture();
    let secret = "provider body with sk-synthetic-sensitive-canary";
    let error = store
        .source_learning_fail(
            SCOPE,
            text(&source, "job_id"),
            text(&source, "lease_token"),
            secret,
        )
        .unwrap_err();
    assert!(!error.to_string().contains(secret));
    let error = store
        .topic_fail(
            SCOPE,
            text(&topic, "job_id"),
            text(&topic, "lease_token"),
            secret,
        )
        .unwrap_err();
    assert!(!error.to_string().contains(secret));
    let connection = Connection::open(store.path()).unwrap();
    for table in ["source_learning_jobs", "topic_jobs"] {
        assert!(
            connection
                .execute(&format!("UPDATE {table} SET failure_code=?1"), [secret])
                .is_err()
        );
        assert!(connection.execute(&format!("UPDATE {table} SET failure_code='authentication_failed',last_error='model_failed' WHERE status='leased'"),[]).is_err());
        for (status, last_error) in [("dead", "NULL"), ("queued", "'model_failed'")] {
            let error = connection.execute(&format!("UPDATE {table} SET status='{status}',attempt_token=NULL,lease_until_ms=NULL,failure_code='authentication_failed',last_error={last_error} WHERE status='leased'"),[]).unwrap_err();
            assert!(
                matches!(error, rusqlite::Error::SqliteFailure(ref code, _) if code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_CHECK)
            );
        }
    }
    assert!(
        statuses(&store, &source, &topic)
            .iter()
            .all(|j| j["status"] == "leased" && j["last_error"].is_null())
    );
    assert!(store.health().unwrap().ok);
}

fn snapshot(
    connection: &Connection,
    table: &str,
    columns: Option<&str>,
) -> (String, Vec<Vec<SqlValue>>) {
    let columns = columns.map(str::to_owned).unwrap_or_else(|| {
        connection
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .map(|name| format!("\"{}\"", name.unwrap().replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(",")
    });
    let mut statement = connection
        .prepare(&format!("SELECT {columns} FROM {table} ORDER BY job_id"))
        .unwrap();
    let count = statement.column_count();
    let rows = statement
        .query_map([], |row| {
            (0..count)
                .map(|i| row.get(i))
                .collect::<rusqlite::Result<Vec<SqlValue>>>()
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    (columns, rows)
}

#[test]
fn schema_thirteen_is_additive_atomic_and_preserves_jobs_and_accepted_topic() {
    for fail_ledger in [false, true] {
        let (temp, store, _source, _topic) = fixture();
        let topic_before = store.topic_get(SCOPE, "published-topic").unwrap();
        let path = store.path().to_owned();
        drop(store);
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(concat!(
                include_str!("fixtures/pre_speaker_schema.sql"),
                include_str!("fixtures/pre_audience_schema.sql")
            ))
            .unwrap();
        connection.execute_batch("DROP TABLE learning_retry_terms; DROP TABLE learning_evidence_retries; ALTER TABLE source_learning_jobs DROP COLUMN failure_code; ALTER TABLE topic_jobs DROP COLUMN failure_code; DELETE FROM schema_migrations WHERE version>=13; PRAGMA user_version=12;").unwrap();
        let before = ["source_learning_jobs", "topic_jobs"].map(|t| snapshot(&connection, t, None));
        if fail_ledger {
            connection.execute_batch("CREATE TRIGGER reject_thirteen BEFORE INSERT ON schema_migrations WHEN NEW.version=13 BEGIN SELECT RAISE(ABORT,'synthetic migration failure'); END;").unwrap();
        }
        drop(connection);
        if fail_ledger {
            assert!(Store::open(&path, 64).is_err());
            let connection = Connection::open(&path).unwrap();
            assert_eq!(
                connection
                    .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                12
            );
            for (table, expected) in ["source_learning_jobs", "topic_jobs"].iter().zip(&before) {
                assert_eq!(&snapshot(&connection, table, None), expected);
            }
            connection
                .execute_batch("DROP TRIGGER reject_thirteen;")
                .unwrap();
        }
        let store = Store::open(&path, 64).unwrap();
        assert_eq!(store.health().unwrap().schema_version, 19);
        assert!(store.health().unwrap().ok);
        assert_eq!(
            store.topic_get(SCOPE, "published-topic").unwrap(),
            topic_before
        );
        let connection = Connection::open(&path).unwrap();
        for (table, (columns, rows)) in ["source_learning_jobs", "topic_jobs"].iter().zip(&before) {
            assert_eq!(&snapshot(&connection, table, Some(columns)).1, rows);
            assert_eq!(
                connection
                    .query_row(
                        &format!("SELECT count(*) FROM {table} WHERE failure_code IS NOT NULL"),
                        [],
                        |r| r.get::<_, i64>(0)
                    )
                    .unwrap(),
                0
            );
        }
        assert!(temp.path().exists());
    }
}
