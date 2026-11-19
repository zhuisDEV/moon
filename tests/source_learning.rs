use moon::source_learning::{SourcePrepareRequest, SourceProposalInput};
use moon::sources::SourceInput;
use moon::{
    ContextRequest, EmbeddingProvider, HashEmbedding, IngestDocument, MemoryInput, SearchMode,
    SearchRequest, Store,
};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::{Arc, Barrier};

const SCOPE: &str = "source-learning-test";
const QUOTE: &str = "Canary uses SQLite for source storage.";

fn open(temp: &tempfile::TempDir) -> Store {
    Store::open(temp.path().join("state/moon.sqlite"), 64).unwrap()
}

fn source_input(key: &str, content: &str) -> SourceInput {
    serde_json::from_value(json!({
        "schema_version":1,"source_key":key,"kind":"external_document",
        "content":content,"author":"Example author"
    }))
    .unwrap()
}

fn capture(store: &mut Store, key: &str, content: &str, eligible: bool) -> Value {
    store
        .source_import(SCOPE, source_input(key, content), None, eligible)
        .unwrap()
}

fn request(key: &str) -> SourcePrepareRequest {
    SourcePrepareRequest {
        scope: SCOPE.into(),
        key: Some(key.into()),
        max_bytes: 65_536,
        lease_ms: 60_000,
        preview: false,
    }
}

fn text<'a>(value: &'a Value, field: &str) -> &'a str {
    value[field].as_str().unwrap()
}

fn proposal(quote: &str) -> SourceProposalInput {
    serde_json::from_value(json!({"schema_version":1,"claims":[{"quote":quote}]})).unwrap()
}

fn submit(store: &mut Store, prepared: &Value, quote: &str) -> Value {
    store
        .source_learning_submit(
            SCOPE,
            text(prepared, "job_id"),
            text(prepared, "lease_token"),
            proposal(quote),
        )
        .unwrap()
}

fn accept(store: &mut Store, prepared: &Value, review: &Value) -> Value {
    store
        .source_learning_accept(SCOPE, text(prepared, "job_id"), text(review, "result_hash"))
        .unwrap()
}

fn learn(store: &mut Store, key: &str, quote: &str) -> (Value, Value, Value) {
    let prepared = store.source_learning_prepare(&request(key)).unwrap();
    assert_eq!(prepared["status"], "prepared");
    let review = submit(store, &prepared, quote);
    let accepted = accept(store, &prepared, &review);
    (prepared, review, accepted)
}

fn claim_id(accepted: &Value) -> &str {
    accepted["claim_ids"][0].as_str().unwrap()
}

fn count(database: &Path, table: &str) -> i64 {
    Connection::open(database)
        .unwrap()
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
}

fn sql(store: &Store, statement: &str) {
    Connection::open(store.path())
        .unwrap()
        .execute_batch(statement)
        .unwrap();
}

fn job(store: &Store, id: &str) -> Value {
    store.source_learning_status(SCOPE, Some(id), 20).unwrap()["jobs"][0].clone()
}

fn active_claims(store: &Store) -> Vec<Value> {
    store.source_claim_list(SCOPE, 100, false).unwrap()["claims"]
        .as_array()
        .unwrap()
        .clone()
}

fn recall(store: &Store) -> usize {
    store
        .search(
            &SearchRequest {
                query: "Canary SQLite source storage".into(),
                mode: SearchMode::Lexical,
                limit: 20,
                scope: Some(SCOPE.into()),
                source_kind: Some("source_claim".into()),
            },
            None,
        )
        .unwrap()
        .len()
}

fn set_withdrawn(store: &mut Store, key: &str, withdrawn: bool) -> Value {
    let state = store.source_state(SCOPE, key).unwrap();
    store
        .source_set_withdrawn(
            SCOPE,
            key,
            text(&state, "revision_id"),
            state["generation"].as_i64().unwrap(),
            withdrawn,
            "test lifecycle change",
        )
        .unwrap()
}

#[test]
fn eligible_intake_queues_atomically_and_review_is_required_before_recall() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = open(&temp);
    capture(&mut store, "reference-only", QUOTE, false);
    assert_eq!(count(store.path(), "source_learning_jobs"), 0);
    let source = capture(&mut store, "eligible", QUOTE, true);
    assert_eq!(count(store.path(), "source_learning_jobs"), 1);
    assert_eq!(recall(&store), 0);
    let prepared = store.source_learning_prepare(&request("eligible")).unwrap();
    assert_eq!(prepared["source"]["revision_id"], source["revision_id"]);
    assert_eq!(prepared["source"]["content"], QUOTE);
    let review = submit(&mut store, &prepared, QUOTE);
    assert_eq!(review["status"], "review");
    assert_eq!(count(store.path(), "documents"), 0);
    assert_eq!(recall(&store), 0);
    assert!(
        store
            .source_learning_accept(SCOPE, text(&prepared, "job_id"), &"0".repeat(64))
            .is_err()
    );
    let accepted = accept(&mut store, &prepared, &review);
    assert_eq!(accepted["status"], "complete");
    assert_eq!(recall(&store), 1);
    assert_eq!(count(store.path(), "source_claims"), 1);
    assert_eq!(count(store.path(), "source_claim_supports"), 1);
    assert_eq!(count(store.path(), "memory_items"), 0);
    assert_eq!(count(store.path(), "evidence_sessions"), 0);
    let saved = store.source_claim_get(SCOPE, claim_id(&accepted)).unwrap();
    assert_eq!(saved["content"], QUOTE);
    assert_eq!(saved["kind"], "author_statement");
    assert_eq!(saved["active"], true);
    assert!(!saved["citations"].as_array().unwrap().is_empty());
    assert_eq!(accept(&mut store, &prepared, &review), accepted);
    assert_eq!(count(store.path(), "documents"), 1);
    assert!(store.health().unwrap().ok);
}

#[test]
fn queue_failure_rolls_back_new_capture_and_eligibility_update() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = open(&temp);
    let first = capture(&mut store, "existing", QUOTE, false);
    sql(
        &store,
        "CREATE TRIGGER test_queue_failure AFTER INSERT ON source_learning_jobs BEGIN SELECT RAISE(ABORT,'synthetic queue failure'); END;",
    );
    assert!(
        store
            .source_import(SCOPE, source_input("new", QUOTE), None, true)
            .is_err()
    );
    assert!(
        store
            .source_import(
                SCOPE,
                source_input("existing", QUOTE),
                Some(text(&first, "revision_id")),
                true
            )
            .is_err()
    );
    assert_eq!(count(store.path(), "sources"), 1);
    assert_eq!(count(store.path(), "source_revisions"), 1);
    assert_eq!(count(store.path(), "source_learning_jobs"), 0);
    assert_eq!(
        store.source_get(SCOPE, "existing", None).unwrap()["learning_eligibility"],
        "reference_only"
    );
}

#[test]
fn malformed_or_ungrounded_model_output_cannot_partially_persist() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = open(&temp);
    capture(&mut store, "grounding", QUOTE, true);
    let prepared = store
        .source_learning_prepare(&request("grounding"))
        .unwrap();
    let bad: SourceProposalInput = serde_json::from_value(json!({
        "schema_version":1,"claims":[{"quote":QUOTE},{"quote":"Invented synthetic-secret-model-answer"}]
    })).unwrap();
    let error = store
        .source_learning_submit(
            SCOPE,
            text(&prepared, "job_id"),
            text(&prepared, "lease_token"),
            bad,
        )
        .unwrap_err();
    assert!(!format!("{error:#}").contains("synthetic-secret-model-answer"));
    assert_eq!(count(store.path(), "source_claims"), 0);
    let connection = Connection::open(store.path()).unwrap();
    let pending: Option<String> = connection
        .query_row(
            "SELECT proposals_json FROM source_learning_jobs",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(pending.is_none());
    assert!(
        serde_json::from_value::<SourceProposalInput>(
            json!({"schema_version":1,"claims":[{"quote":QUOTE,"content":"rewrite this claim"}]})
        )
        .is_err()
    );
    let review = submit(&mut store, &prepared, QUOTE);
    accept(&mut store, &prepared, &review);
    assert_eq!(active_claims(&store).len(), 1);
}

#[test]
fn repeated_unicode_quotes_require_explicit_valid_byte_location() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = open(&temp);
    let quote = "猫 uses SQLite.";
    let body = format!("{quote}\n{quote}");
    capture(&mut store, "unicode", &body, true);
    let prepared = store.source_learning_prepare(&request("unicode")).unwrap();
    assert!(
        store
            .source_learning_submit(
                SCOPE,
                text(&prepared, "job_id"),
                text(&prepared, "lease_token"),
                proposal(quote)
            )
            .is_err()
    );
    let invalid: SourceProposalInput = serde_json::from_value(
        json!({"schema_version":1,"claims":[{"quote":quote,"start_byte":1}]}),
    )
    .unwrap();
    assert!(
        store
            .source_learning_submit(
                SCOPE,
                text(&prepared, "job_id"),
                text(&prepared, "lease_token"),
                invalid
            )
            .is_err()
    );
    let exact: SourceProposalInput = serde_json::from_value(
        json!({"schema_version":1,"claims":[{"quote":quote,"start_byte":quote.len()+1}]}),
    )
    .unwrap();
    let review = store
        .source_learning_submit(
            SCOPE,
            text(&prepared, "job_id"),
            text(&prepared, "lease_token"),
            exact,
        )
        .unwrap();
    let accepted = accept(&mut store, &prepared, &review);
    let connection = Connection::open(store.path()).unwrap();
    let (start, line): (i64, i64) = connection
        .query_row(
            "SELECT start_byte,start_line FROM source_claim_supports WHERE claim_id=?1",
            [claim_id(&accepted)],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(start, quote.len() as i64 + 1);
    assert_eq!(line, 2);
}

#[test]
fn preview_and_input_budget_do_not_take_a_lease_or_consume_attempts() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = open(&temp);
    capture(&mut store, "bounded", &"x".repeat(2048), true);
    let mut preview = request("bounded");
    preview.preview = true;
    store.source_learning_prepare(&preview).unwrap();
    let connection = Connection::open(store.path()).unwrap();
    let (status, attempts, token): (String, i64, Option<String>) = connection
        .query_row(
            "SELECT status,attempts,attempt_token FROM source_learning_jobs",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(status, "queued");
    assert_eq!(attempts, 0);
    assert!(token.is_none());
    let mut bounded = request("bounded");
    bounded.max_bytes = 1024;
    assert_eq!(
        store.source_learning_prepare(&bounded).unwrap()["status"],
        "input_too_large"
    );
    let attempts: i64 = connection
        .query_row("SELECT attempts FROM source_learning_jobs", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(attempts, 0);
    assert_eq!(
        store.source_learning_prepare(&request("bounded")).unwrap()["status"],
        "prepared"
    );
}

#[test]
fn expired_worker_lease_recovers_after_reopen_and_old_worker_is_fenced() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = open(&temp);
    capture(&mut store, "crash", QUOTE, true);
    let first = store.source_learning_prepare(&request("crash")).unwrap();
    assert_eq!(
        store.source_learning_prepare(&request("crash")).unwrap()["status"],
        "busy"
    );
    let path = store.path().to_path_buf();
    drop(store); // Simulate loss of the worker without submitting a result.
    let mut store = Store::open(&path, 64).unwrap();
    sql(
        &store,
        "UPDATE source_learning_jobs SET lease_until_ms=1 WHERE status='leased';",
    );
    let second = store.source_learning_prepare(&request("crash")).unwrap();
    assert_eq!(first["job_id"], second["job_id"]);
    assert_ne!(first["lease_token"], second["lease_token"]);
    assert!(
        store
            .source_learning_submit(
                SCOPE,
                text(&first, "job_id"),
                text(&first, "lease_token"),
                proposal(QUOTE)
            )
            .is_err()
    );
    assert!(
        store
            .source_learning_fail(
                SCOPE,
                text(&first, "job_id"),
                text(&first, "lease_token"),
                "model_failed"
            )
            .is_err()
    );
    let review = submit(&mut store, &second, QUOTE);
    accept(&mut store, &second, &review);
    assert_eq!(job(&store, text(&second, "job_id"))["status"], "complete");
    assert!(store.health().unwrap().ok);
}

#[test]
fn model_failures_are_bounded_and_only_explicit_retry_revives_dead_work() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = open(&temp);
    capture(&mut store, "failures", QUOTE, true);
    let mut id = String::new();
    for attempt in 1..=3 {
        sql(
            &store,
            "UPDATE source_learning_jobs SET not_before_ms=0 WHERE status='queued';",
        );
        let prepared = store.source_learning_prepare(&request("failures")).unwrap();
        id = text(&prepared, "job_id").to_owned();
        assert!(
            store
                .source_learning_fail(
                    SCOPE,
                    &id,
                    text(&prepared, "lease_token"),
                    "synthetic-secret-provider-error"
                )
                .is_err()
        );
        store
            .source_learning_fail(SCOPE, &id, text(&prepared, "lease_token"), "model_failed")
            .unwrap();
        assert_eq!(job(&store, &id)["attempts"], attempt);
    }
    assert_eq!(job(&store, &id)["status"], "dead");
    assert_ne!(
        store.source_learning_prepare(&request("failures")).unwrap()["status"],
        "prepared"
    );
    store.source_learning_retry(SCOPE, &id).unwrap();
    let retry = store.source_learning_prepare(&request("failures")).unwrap();
    assert_eq!(retry["status"], "prepared");
    assert_eq!(job(&store, &id)["attempts"], 1);
    assert_eq!(count(store.path(), "documents"), 0);
}

#[test]
fn source_revision_change_cancels_review_and_immediately_hides_old_claims() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = open(&temp);
    let original = capture(&mut store, "revision", QUOTE, true);
    let (_, _, accepted) = learn(&mut store, "revision", QUOTE);
    assert_eq!(recall(&store), 1);
    let changed = "Canary uses another local database.";
    let newer = store
        .source_import(
            SCOPE,
            source_input("revision", changed),
            Some(text(&original, "revision_id")),
            true,
        )
        .unwrap();
    assert_eq!(active_claims(&store).len(), 0);
    assert_eq!(recall(&store), 0);
    assert_eq!(
        store.source_claim_get(SCOPE, claim_id(&accepted)).unwrap()["active"],
        false
    );
    let prepared = store.source_learning_prepare(&request("revision")).unwrap();
    assert_eq!(prepared["source"]["revision_id"], newer["revision_id"]);
    let review = submit(&mut store, &prepared, changed);
    store
        .source_import(
            SCOPE,
            source_input("revision", "A third revision."),
            Some(text(&newer, "revision_id")),
            true,
        )
        .unwrap();
    assert!(
        store
            .source_learning_accept(
                SCOPE,
                text(&prepared, "job_id"),
                text(&review, "result_hash")
            )
            .is_err()
    );
    assert_eq!(
        job(&store, text(&prepared, "job_id"))["status"],
        "cancelled"
    );
    assert!(store.health().unwrap().ok);
}

#[test]
fn withdrawal_is_generation_checked_and_restoration_revives_valid_accepted_support() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = open(&temp);
    capture(&mut store, "withdraw", QUOTE, true);
    let (_, _, accepted) = learn(&mut store, "withdraw", QUOTE);
    let original = store.source_state(SCOPE, "withdraw").unwrap();
    let withdrawn = set_withdrawn(&mut store, "withdraw", true);
    assert_eq!(withdrawn["status"], "withdrawn");
    assert_eq!(recall(&store), 0);
    assert!(
        store
            .source_set_withdrawn(
                SCOPE,
                "withdraw",
                text(&original, "revision_id"),
                original["generation"].as_i64().unwrap(),
                false,
                "stale restore"
            )
            .is_err()
    );
    let restored = set_withdrawn(&mut store, "withdraw", false);
    assert_eq!(restored["status"], "active");
    assert_eq!(recall(&store), 1);
    assert_eq!(
        store.source_claim_get(SCOPE, claim_id(&accepted)).unwrap()["active"],
        true
    );
}

#[test]
fn independent_identical_support_survives_one_source_withdrawal_and_retains_attribution() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = open(&temp);
    capture(&mut store, "one", QUOTE, true);
    capture(&mut store, "two", QUOTE, true);
    let (_, _, a) = learn(&mut store, "one", QUOTE);
    let (_, _, b) = learn(&mut store, "two", QUOTE);
    assert_eq!(claim_id(&a), claim_id(&b));
    assert_eq!(count(store.path(), "source_claim_supports"), 2);
    set_withdrawn(&mut store, "one", true);
    assert_eq!(recall(&store), 1);
    set_withdrawn(&mut store, "two", true);
    assert_eq!(recall(&store), 0);
    let mut another = source_input("different-author", QUOTE);
    another.author = Some("Different author".into());
    store.source_import(SCOPE, another, None, true).unwrap();
    let (_, _, different) = learn(&mut store, "different-author", QUOTE);
    assert_eq!(claim_id(&a), claim_id(&different));
    let context = store
        .assemble_context(
            &ContextRequest {
                query: "Canary SQLite source storage".into(),
                mode: SearchMode::Lexical,
                limit: 8,
                scope: Some(SCOPE.into()),
                max_chars: 3500,
                evidence_per_memory: 2,
            },
            None,
        )
        .unwrap();
    let attributed = context
        .references
        .iter()
        .find_map(|reference| reference.source_claim.as_ref())
        .unwrap();
    assert!(
        attributed
            .citations
            .iter()
            .any(|citation| citation.author.as_deref() == Some("Different author"))
    );
    let mut user = source_input("user", QUOTE);
    user.kind = moon::sources::SourceKind::UserStatement;
    store.source_import(SCOPE, user, None, true).unwrap();
    let (_, _, user_claim) = learn(&mut store, "user", QUOTE);
    assert_ne!(claim_id(&a), claim_id(&user_claim));
    assert_eq!(
        store
            .source_claim_get(SCOPE, claim_id(&user_claim))
            .unwrap()["kind"],
        "user_statement"
    );
}

#[test]
fn scoped_job_claim_and_lifecycle_operations_cannot_cross_read_or_mutate() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = open(&temp);
    let captured = capture(&mut store, "scope", QUOTE, true);
    let prepared = store.source_learning_prepare(&request("scope")).unwrap();
    assert!(
        store
            .source_learning_submit(
                "foreign",
                text(&prepared, "job_id"),
                text(&prepared, "lease_token"),
                proposal(QUOTE)
            )
            .is_err()
    );
    assert!(
        store
            .source_learning_status("foreign", Some(text(&prepared, "job_id")), 20)
            .is_err()
    );
    assert!(
        store
            .source_learning_fail(
                "foreign",
                text(&prepared, "job_id"),
                text(&prepared, "lease_token"),
                "model_failed"
            )
            .is_err()
    );
    let review = submit(&mut store, &prepared, QUOTE);
    assert!(
        store
            .source_learning_accept(
                "foreign",
                text(&prepared, "job_id"),
                text(&review, "result_hash")
            )
            .is_err()
    );
    let accepted = accept(&mut store, &prepared, &review);
    assert!(
        store
            .source_claim_get("foreign", claim_id(&accepted))
            .is_err()
    );
    assert!(
        store.source_claim_list("foreign", 20, false).unwrap()["claims"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .source_set_withdrawn(
                "foreign",
                "scope",
                text(&captured, "revision_id"),
                1,
                true,
                "wrong scope"
            )
            .is_err()
    );
    assert!(store.source_delete_preview("foreign", "scope").is_err());
    assert_eq!(recall(&store), 1);
}

#[test]
fn claim_acceptance_rolls_back_document_support_and_job_state_on_failure() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = open(&temp);
    capture(&mut store, "atomic", QUOTE, true);
    let prepared = store.source_learning_prepare(&request("atomic")).unwrap();
    let review = submit(&mut store, &prepared, QUOTE);
    sql(
        &store,
        "CREATE TRIGGER test_accept_failure AFTER INSERT ON source_claim_supports BEGIN SELECT RAISE(ABORT,'synthetic acceptance failure'); END;",
    );
    assert!(
        store
            .source_learning_accept(
                SCOPE,
                text(&prepared, "job_id"),
                text(&review, "result_hash")
            )
            .is_err()
    );
    assert_eq!(count(store.path(), "documents"), 0);
    assert_eq!(count(store.path(), "source_claims"), 0);
    assert_eq!(count(store.path(), "source_claim_supports"), 0);
    assert_eq!(job(&store, text(&prepared, "job_id"))["status"], "review");
    sql(&store, "DROP TRIGGER test_accept_failure;");
    accept(&mut store, &prepared, &review);
    assert_eq!(recall(&store), 1);
    assert!(store.health().unwrap().ok);
}

#[test]
fn concurrent_acceptance_of_one_review_is_idempotent() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = open(&temp);
    capture(&mut store, "accept-race", QUOTE, true);
    let prepared = store
        .source_learning_prepare(&request("accept-race"))
        .unwrap();
    let review = submit(&mut store, &prepared, QUOTE);
    let path = store.path().to_path_buf();
    drop(store);
    let barrier = Arc::new(Barrier::new(2));
    let workers = [0, 1].map(|_| {
        let path = path.clone();
        let barrier = Arc::clone(&barrier);
        let id = text(&prepared, "job_id").to_owned();
        let hash = text(&review, "result_hash").to_owned();
        std::thread::spawn(move || {
            let mut store = Store::open(path, 64).unwrap();
            barrier.wait();
            store.source_learning_accept(SCOPE, &id, &hash).unwrap()
        })
    });
    let [a, b] = workers.map(|worker| worker.join().unwrap());
    assert_eq!(a, b);
    assert_eq!(count(&path, "documents"), 1);
    assert_eq!(count(&path, "source_claims"), 1);
    assert_eq!(count(&path, "source_claim_supports"), 1);
}

#[test]
fn expiration_is_checked_at_read_time_without_a_maintenance_tick() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = open(&temp);
    capture(&mut store, "expiry", QUOTE, true);
    let prepared = store.source_learning_prepare(&request("expiry")).unwrap();
    let future = chrono::Utc::now().timestamp_millis() + 60_000;
    let expiring: SourceProposalInput = serde_json::from_value(
        json!({"schema_version":1,"claims":[{"quote":QUOTE,"valid_until_ms":future}]}),
    )
    .unwrap();
    let review = store
        .source_learning_submit(
            SCOPE,
            text(&prepared, "job_id"),
            text(&prepared, "lease_token"),
            expiring,
        )
        .unwrap();
    let accepted = accept(&mut store, &prepared, &review);
    assert_eq!(recall(&store), 1);
    // Advance the fixture's expiry, not the wall clock or a maintenance worker.
    sql(
        &store,
        "DROP TRIGGER source_claims_immutable_update; UPDATE source_claims SET valid_until_ms=1;",
    );
    assert_eq!(recall(&store), 0);
    assert!(active_claims(&store).is_empty());
    assert_eq!(
        store.source_claim_get(SCOPE, claim_id(&accepted)).unwrap()["active"],
        false
    );
}

#[test]
fn deletion_plan_detects_changed_jobs_and_removes_content_with_a_tombstone() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = open(&temp);
    capture(&mut store, "delete", QUOTE, true);
    let before = store.source_delete_preview(SCOPE, "delete").unwrap();
    let (prepared, review, accepted) = learn(&mut store, "delete", QUOTE);
    assert!(
        store
            .source_delete_apply(SCOPE, "delete", text(&before, "plan_hash"))
            .is_err()
    );
    assert_eq!(recall(&store), 1);
    let current = store.source_delete_preview(SCOPE, "delete").unwrap();
    assert_ne!(current["plan_hash"], before["plan_hash"]);
    store
        .source_delete_apply(SCOPE, "delete", text(&current, "plan_hash"))
        .unwrap();
    assert_eq!(
        store.source_state(SCOPE, "delete").unwrap()["status"],
        "deleted"
    );
    assert!(store.source_get(SCOPE, "delete", None).is_err());
    assert!(store.source_claim_get(SCOPE, claim_id(&accepted)).is_err());
    assert_eq!(count(store.path(), "source_revisions"), 0);
    assert_eq!(count(store.path(), "source_claims"), 0);
    assert_eq!(count(store.path(), "source_claim_supports"), 0);
    assert_eq!(count(store.path(), "source_learning_jobs"), 0);
    assert_eq!(count(store.path(), "documents"), 0);
    assert_eq!(count(store.path(), "sources"), 1);
    assert_eq!(recall(&store), 0);
    assert!(
        store
            .source_learning_accept(
                SCOPE,
                text(&prepared, "job_id"),
                text(&review, "result_hash")
            )
            .is_err()
    );
    assert!(
        store
            .source_import(SCOPE, source_input("delete", QUOTE), None, true)
            .is_err()
    );
    assert!(store.health().unwrap().ok);
}

#[test]
fn deleting_one_support_preserves_a_claim_supported_by_another_live_source() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = open(&temp);
    capture(&mut store, "one", QUOTE, true);
    capture(&mut store, "two", QUOTE, true);
    let (_, _, a) = learn(&mut store, "one", QUOTE);
    let (_, _, b) = learn(&mut store, "two", QUOTE);
    assert_eq!(claim_id(&a), claim_id(&b));
    let plan = store.source_delete_preview(SCOPE, "one").unwrap();
    store
        .source_delete_apply(SCOPE, "one", text(&plan, "plan_hash"))
        .unwrap();
    assert_eq!(recall(&store), 1);
    assert_eq!(count(store.path(), "source_claim_supports"), 1);
    assert_eq!(
        store.source_claim_get(SCOPE, claim_id(&b)).unwrap()["active"],
        true
    );
    assert!(store.health().unwrap().ok);
}

#[test]
fn deletion_failure_rolls_back_lifecycle_claims_and_indexed_content() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = open(&temp);
    capture(&mut store, "delete-rollback", QUOTE, true);
    learn(&mut store, "delete-rollback", QUOTE);
    let plan = store
        .source_delete_preview(SCOPE, "delete-rollback")
        .unwrap();
    sql(
        &store,
        "CREATE TRIGGER test_delete_failure BEFORE DELETE ON source_revisions BEGIN SELECT RAISE(ABORT,'synthetic deletion failure'); END;",
    );
    assert!(
        store
            .source_delete_apply(SCOPE, "delete-rollback", text(&plan, "plan_hash"))
            .is_err()
    );
    assert_eq!(
        store.source_state(SCOPE, "delete-rollback").unwrap()["status"],
        "active"
    );
    assert_eq!(
        store.source_get(SCOPE, "delete-rollback", None).unwrap()["content"],
        QUOTE
    );
    assert_eq!(recall(&store), 1);
    assert_eq!(count(store.path(), "source_learning_jobs"), 1);
    assert!(store.health().unwrap().ok);
}

#[test]
fn backup_restore_preserves_review_and_accepted_support_lifecycle() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = open(&temp);
    capture(&mut store, "accepted", QUOTE, true);
    let (_, _, accepted) = learn(&mut store, "accepted", QUOTE);
    capture(
        &mut store,
        "pending",
        "Another bounded source statement.",
        true,
    );
    let pending = store.source_learning_prepare(&request("pending")).unwrap();
    let review = submit(&mut store, &pending, "Another bounded source statement.");
    let backup = temp.path().join("backup.sqlite");
    store.backup_to(&backup).unwrap();
    let mut restored = Store::open(&backup, 64).unwrap();
    assert_eq!(
        restored
            .source_claim_get(SCOPE, claim_id(&accepted))
            .unwrap()["active"],
        true
    );
    assert_eq!(job(&restored, text(&pending, "job_id"))["status"], "review");
    accept(&mut restored, &pending, &review);
    assert_eq!(active_claims(&restored).len(), 2);
    assert_eq!(active_claims(&store).len(), 1);
    assert!(restored.health().unwrap().ok);
}

#[test]
fn concurrent_workers_cannot_both_lease_the_same_source_job() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = open(&temp);
    capture(&mut store, "lease-race", QUOTE, true);
    let path = store.path().to_path_buf();
    drop(store);
    let barrier = Arc::new(Barrier::new(2));
    let workers = [0, 1].map(|_| {
        let path = path.clone();
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || {
            let mut store = Store::open(path, 64).unwrap();
            barrier.wait();
            store
                .source_learning_prepare(&request("lease-race"))
                .unwrap()
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
    let connection = Connection::open(path).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT attempts FROM source_learning_jobs", [], |row| row
                .get::<_, i64>(
                0
            ))
            .unwrap(),
        1
    );
}

#[test]
fn public_ingestion_cannot_forge_source_claim_storage_or_its_reserved_uri() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = open(&temp);
    let document = |uri: &str, kind: &str| IngestDocument {
        source_uri: uri.into(),
        source_kind: kind.into(),
        scope: SCOPE.into(),
        title: None,
        content: QUOTE.into(),
        modified_at_ms: 100,
        metadata_json: "{}".into(),
    };
    assert!(
        store
            .ingest(document("test://forged", "source_claim"))
            .is_err()
    );
    for uri in [
        "moon-source-claim://forged",
        "MOON-SOURCE-CLAIM://forged",
        "MoOn-SoUrCe-ClAiM://forged",
    ] {
        assert!(store.ingest(document(uri, "library")).is_err());
    }
    assert!(
        store
            .remember(MemoryInput {
                memory_kind: "source_claim".into(),
                scope: SCOPE.into(),
                title: None,
                content: QUOTE.into(),
                importance: 0.5,
                confidence: 1.0,
                pinned: false,
            })
            .is_err()
    );
    assert_eq!(count(store.path(), "documents"), 0);
    assert_eq!(count(store.path(), "source_claims"), 0);
    assert!(store.health().unwrap().ok);
}

struct WithdrawDuringEmbedding {
    database: std::path::PathBuf,
}

impl EmbeddingProvider for WithdrawDuringEmbedding {
    fn name(&self) -> &str {
        "hash"
    }
    fn model(&self) -> &str {
        "moon-hash-v1"
    }
    fn dimensions(&self) -> usize {
        64
    }
    fn embed_documents(&self, inputs: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        let mut store = Store::open(&self.database, 64)?;
        set_withdrawn(&mut store, "embedding-race", true);
        HashEmbedding::new(64).embed_documents(inputs)
    }
    fn embed_query(&self, input: &str) -> anyhow::Result<Vec<f32>> {
        HashEmbedding::new(64).embed_query(input)
    }
}

#[test]
fn invalidated_source_cannot_write_back_an_inflight_embedding_and_restore_requeues() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = open(&temp);
    capture(&mut store, "embedding-race", QUOTE, true);
    learn(&mut store, "embedding-race", QUOTE);
    let provider = WithdrawDuringEmbedding {
        database: store.path().to_path_buf(),
    };
    let report = store.embed_pending(&provider, 16).unwrap();
    assert_eq!(report.selected, 1);
    assert_eq!(report.embedded, 0);
    assert_eq!(count(store.path(), "chunk_vectors"), 0);
    assert_eq!(count(store.path(), "embedding_queue"), 0);
    assert_eq!(recall(&store), 0);
    set_withdrawn(&mut store, "embedding-race", false);
    assert_eq!(count(store.path(), "embedding_queue"), 1);
    assert_eq!(
        store
            .embed_pending(&HashEmbedding::new(64), 16)
            .unwrap()
            .embedded,
        1
    );
    let hits = store
        .search(
            &SearchRequest {
                query: "Canary SQLite source storage".into(),
                mode: SearchMode::Semantic,
                limit: 1,
                scope: Some(SCOPE.into()),
                source_kind: Some("source_claim".into()),
            },
            Some(&HashEmbedding::new(64)),
        )
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert!(store.health().unwrap().ok);
}

fn schema_nine(database: &Path, records: &[Value]) {
    let connection = Connection::open(database).unwrap();
    for (index, migration) in [
        include_str!("../migrations/0001_init.sql"),
        include_str!("../migrations/0002_canonical_body.sql"),
        include_str!("../migrations/0003_fts_source_kind.sql"),
        include_str!("../migrations/0004_evidence_memory.sql"),
        include_str!("../migrations/0005_runtime_safety.sql"),
        include_str!("../migrations/0006_auto_embedding.sql"),
        include_str!("../migrations/0007_context_metrics.sql"),
        include_str!("../migrations/0008_learning_lifecycle.sql"),
        include_str!("../migrations/0009_source_storage.sql"),
    ]
    .into_iter()
    .enumerate()
    {
        connection.execute_batch(migration).unwrap();
        connection
            .execute(
                "INSERT INTO schema_migrations(version,applied_at_ms) VALUES(?1,1)",
                [(index + 1) as i64],
            )
            .unwrap();
    }
    connection.execute_batch("INSERT INTO metadata(key,value) VALUES('embedding_dimensions','64'); CREATE VIRTUAL TABLE chunk_vectors USING vec0(embedding float[64],source_kind text,scope text); PRAGMA user_version=9;").unwrap();
    for record in records {
        connection.execute("INSERT OR IGNORE INTO sources(source_id,scope,source_key,created_at_ms) VALUES(?1,?2,?3,?4)", params![
            text(record,"source_id"),text(record,"scope"),text(record,"source_key"),record["captured_at_ms"].as_i64().unwrap()
        ]).unwrap();
        connection.execute("INSERT INTO source_revisions(revision_id,source_id,revision,kind,format,learning_eligibility,origin,title,author,content,content_hash,revision_hash,published_at_ms,captured_at_ms,redactions,extraction_version)
            VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",params![
            text(record,"revision_id"),text(record,"source_id"),record["revision"].as_i64().unwrap(),text(record,"kind"),text(record,"format"),text(record,"learning_eligibility"),
            record["origin"].as_str(),record["title"].as_str(),record["author"].as_str(),text(record,"content"),text(record,"content_hash"),text(record,"revision_hash"),
            record["published_at_ms"].as_i64(),record["captured_at_ms"].as_i64().unwrap(),record["redactions"].as_i64().unwrap(),text(record,"extraction_version")
        ]).unwrap();
    }
}

#[test]
fn schema_nine_upgrade_queues_only_current_eligible_heads_and_preserves_old_citations() {
    let temp = tempfile::tempdir().unwrap();
    let mut fixture = open(&temp);
    let mut records = Vec::new();
    let old = capture(&mut fixture, "eligible", QUOTE, true);
    records.push(fixture.source_get(SCOPE, "eligible", None).unwrap());
    let revised_quote = "Canary uses SQLite with explicit source revisions.";
    let latest = fixture
        .source_import(
            SCOPE,
            source_input("eligible", revised_quote),
            Some(text(&old, "revision_id")),
            true,
        )
        .unwrap();
    records.push(fixture.source_get(SCOPE, "eligible", None).unwrap());
    capture(&mut fixture, "reference", QUOTE, false);
    records.push(fixture.source_get(SCOPE, "reference", None).unwrap());
    let initially_eligible = capture(&mut fixture, "downgraded", QUOTE, true);
    records.push(fixture.source_get(SCOPE, "downgraded", None).unwrap());
    fixture
        .source_import(
            SCOPE,
            source_input("downgraded", QUOTE),
            Some(text(&initially_eligible, "revision_id")),
            false,
        )
        .unwrap();
    records.push(fixture.source_get(SCOPE, "downgraded", None).unwrap());
    let citation = fixture
        .source_cite(SCOPE, "eligible", text(&old, "revision_id"), 0, QUOTE.len())
        .unwrap();
    let old_database = temp.path().join("schema-nine.sqlite");
    schema_nine(&old_database, &records);
    let mut upgraded = Store::open(&old_database, 64).unwrap();
    assert_eq!(count(&old_database, "source_lifecycle"), 3);
    assert_eq!(count(&old_database, "source_learning_jobs"), 1);
    assert_eq!(count(&old_database, "source_revisions"), 5);
    assert_eq!(
        upgraded
            .source_cite(SCOPE, "eligible", text(&old, "revision_id"), 0, QUOTE.len())
            .unwrap(),
        citation
    );
    let prepared = upgraded
        .source_learning_prepare(&request("eligible"))
        .unwrap();
    assert_eq!(prepared["source"]["revision_id"], latest["revision_id"]);
    assert_eq!(prepared["source"]["content"], revised_quote);
    assert_eq!(
        upgraded
            .source_learning_prepare(&request("reference"))
            .unwrap()["status"],
        "idle"
    );
    assert_eq!(
        upgraded
            .source_learning_prepare(&request("downgraded"))
            .unwrap()["status"],
        "idle"
    );
    let review = submit(&mut upgraded, &prepared, revised_quote);
    accept(&mut upgraded, &prepared, &review);
    assert!(upgraded.health().unwrap().ok);
    drop(upgraded);
    let reopened = Store::open(&old_database, 64).unwrap();
    assert_eq!(count(&old_database, "source_learning_jobs"), 1);
    assert_eq!(reopened.health().unwrap().schema_version, 20);
}

#[test]
fn deleting_one_expired_shared_support_retains_the_other_source_and_inactive_claim() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = open(&temp);
    let mut ids = Vec::new();
    for key in ["expired-one", "expired-two"] {
        capture(&mut store, key, QUOTE, true);
        let prepared = store.source_learning_prepare(&request(key)).unwrap();
        let expired: SourceProposalInput = serde_json::from_value(json!({
            "schema_version":1,"claims":[{"quote":QUOTE,"valid_until_ms":1}]
        }))
        .unwrap();
        let review = store
            .source_learning_submit(
                SCOPE,
                text(&prepared, "job_id"),
                text(&prepared, "lease_token"),
                expired,
            )
            .unwrap();
        let accepted = accept(&mut store, &prepared, &review);
        ids.push(claim_id(&accepted).to_owned());
    }
    assert_eq!(ids[0], ids[1]);
    assert_eq!(recall(&store), 0);
    let preview = store.source_delete_preview(SCOPE, "expired-one").unwrap();
    store
        .source_delete_apply(SCOPE, "expired-one", text(&preview, "plan_hash"))
        .unwrap();
    assert_eq!(count(store.path(), "source_claims"), 1);
    assert_eq!(count(store.path(), "source_claim_supports"), 1);
    assert_eq!(count(store.path(), "documents"), 1);
    assert_eq!(
        store.source_claim_get(SCOPE, &ids[1]).unwrap()["active"],
        false
    );
    assert_eq!(
        store.source_get(SCOPE, "expired-two", None).unwrap()["content"],
        QUOTE
    );
    assert!(store.health().unwrap().ok);
}

#[test]
fn overlapping_ascii_and_unicode_quotes_are_ambiguous_without_an_exact_offset() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = open(&temp);
    for (key, body, quote, offset) in [
        ("ascii-overlap", "banana", "ana", 3),
        ("unicode-overlap", "猫猫猫", "猫猫", 3),
    ] {
        capture(&mut store, key, body, true);
        let prepared = store.source_learning_prepare(&request(key)).unwrap();
        assert!(
            store
                .source_learning_submit(
                    SCOPE,
                    text(&prepared, "job_id"),
                    text(&prepared, "lease_token"),
                    proposal(quote)
                )
                .is_err()
        );
        assert_eq!(job(&store, text(&prepared, "job_id"))["status"], "leased");
        let located: SourceProposalInput = serde_json::from_value(json!({
            "schema_version":1,"claims":[{"quote":quote,"start_byte":offset}]
        }))
        .unwrap();
        let review = store
            .source_learning_submit(
                SCOPE,
                text(&prepared, "job_id"),
                text(&prepared, "lease_token"),
                located,
            )
            .unwrap();
        let accepted = accept(&mut store, &prepared, &review);
        let claim = store.source_claim_get(SCOPE, claim_id(&accepted)).unwrap();
        assert_eq!(claim["content"], quote);
        assert_eq!(claim["citations"][0]["start_byte"], offset);
        assert_eq!(claim["citations"][0]["end_byte"], offset + quote.len());
    }
}

#[test]
fn an_oversized_older_job_does_not_starve_a_later_fitting_source() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = open(&temp);
    let large = capture(&mut store, "oversized-first", &"x".repeat(2048), true);
    capture(&mut store, "fits-later", QUOTE, true);
    let connection = Connection::open(store.path()).unwrap();
    connection.execute("UPDATE source_learning_jobs SET created_at_ms=CASE WHEN source_id=?1 THEN 1 ELSE 2 END",[text(&large,"source_id")]).unwrap();
    let request = SourcePrepareRequest {
        scope: SCOPE.into(),
        key: None,
        max_bytes: 1024,
        lease_ms: 60_000,
        preview: false,
    };
    let prepared = store.source_learning_prepare(&request).unwrap();
    assert_eq!(prepared["status"], "prepared");
    assert_eq!(prepared["source"]["source_key"], "fits-later");
    let review = submit(&mut store, &prepared, QUOTE);
    accept(&mut store, &prepared, &review);
    let remaining = store.source_learning_prepare(&request).unwrap();
    assert_eq!(remaining["status"], "input_too_large");
    let (status, attempts): (String, i64) = connection
        .query_row(
            "SELECT status,attempts FROM source_learning_jobs WHERE source_id=?1",
            [text(&large, "source_id")],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "queued");
    assert_eq!(attempts, 0);
}

#[test]
fn expired_cached_claims_cannot_crowd_a_current_claim_out_of_limit_one_recall() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = open(&temp);
    let future = chrono::Utc::now().timestamp_millis() + 86_400_000;
    for index in 0..6 {
        let key = format!("expired-{index}");
        capture(&mut store, &key, QUOTE, true);
        let prepared = store.source_learning_prepare(&request(&key)).unwrap();
        // Distinct expiry identities retain separate cached documents despite
        // identical wording. All six rank above the longer current statement.
        let expiring: SourceProposalInput = serde_json::from_value(json!({
            "schema_version":1,"claims":[{"quote":QUOTE,"valid_until_ms":future+index}]
        }))
        .unwrap();
        let review = store
            .source_learning_submit(
                SCOPE,
                text(&prepared, "job_id"),
                text(&prepared, "lease_token"),
                expiring,
            )
            .unwrap();
        accept(&mut store, &prepared, &review);
    }
    let current_quote = format!(
        "{QUOTE} This source also explains revision histories, attribution, maintenance and operating constraints."
    );
    capture(&mut store, "current-lower-rank", &current_quote, true);
    let (_, _, accepted) = learn(&mut store, "current-lower-rank", &current_quote);
    let current = store.source_claim_get(SCOPE, claim_id(&accepted)).unwrap();
    let provider = HashEmbedding::new(64);
    assert_eq!(store.embed_pending(&provider, 100).unwrap().embedded, 7);
    sql(&store, "DROP TRIGGER source_claims_immutable_update;");
    let connection = Connection::open(store.path()).unwrap();
    connection
        .execute(
            "UPDATE source_claims SET valid_until_ms=1 WHERE claim_id<>?1",
            [claim_id(&accepted)],
        )
        .unwrap();
    // No cleanup or index refresh occurs after fixture expiry: stale FTS and
    // vector entries remain present, as they can between maintenance ticks.
    assert_eq!(count(store.path(), "chunk_vectors"), 7);
    assert_eq!(count(store.path(), "chunk_fts"), 7);
    for mode in [
        SearchMode::Lexical,
        SearchMode::Semantic,
        SearchMode::Hybrid,
    ] {
        let hits = store
            .search(
                &SearchRequest {
                    query: QUOTE.into(),
                    mode,
                    limit: 1,
                    scope: Some(SCOPE.into()),
                    source_kind: Some("source_claim".into()),
                },
                Some(&provider),
            )
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0].document_id,
            current["document_id"].as_i64().unwrap()
        );
        assert_eq!(hits[0].content, current_quote);
    }
}

#[test]
fn lifecycle_history_retains_withdrawal_reasons_after_restore_and_purges_selected_reasons_on_delete()
 {
    let temp = tempfile::tempdir().unwrap();
    let mut store = open(&temp);
    capture(&mut store, "history", QUOTE, false);
    let first = store.source_state(SCOPE, "history").unwrap();
    assert_eq!(first["history"].as_array().unwrap().len(), 1);
    assert_eq!(first["history"][0]["action"], "capture");
    store
        .source_set_withdrawn(
            SCOPE,
            "history",
            text(&first, "revision_id"),
            1,
            true,
            "Private research rationale; password=synthetic-history-secret",
        )
        .unwrap();
    let withdrawn = store.source_state(SCOPE, "history").unwrap();
    store
        .source_set_withdrawn(
            SCOPE,
            "history",
            text(&withdrawn, "revision_id"),
            2,
            false,
            "Restored after source review",
        )
        .unwrap();
    let restored = store.source_state(SCOPE, "history").unwrap();
    assert_eq!(restored["history"].as_array().unwrap().len(), 3);
    assert_eq!(restored["history"][0]["action"], "restore");
    assert_eq!(restored["history"][1]["action"], "withdraw");
    assert!(text(&restored["history"][1], "reason").contains("Private research rationale"));
    assert!(!restored.to_string().contains("synthetic-history-secret"));
    capture(&mut store, "unrelated-history", "Unrelated source.", false);
    set_withdrawn(&mut store, "unrelated-history", true);
    let unrelated = store.source_state(SCOPE, "unrelated-history").unwrap();
    let plan = store.source_delete_preview(SCOPE, "history").unwrap();
    store
        .source_delete_apply(SCOPE, "history", text(&plan, "plan_hash"))
        .unwrap();
    let tombstone = store.source_state(SCOPE, "history").unwrap();
    assert_eq!(tombstone["status"], "deleted");
    assert_eq!(tombstone["history"].as_array().unwrap().len(), 1);
    assert_eq!(tombstone["history"][0]["action"], "delete");
    assert_eq!(tombstone["history"][0]["reason"], "deleted");
    assert!(!tombstone.to_string().contains("Private research rationale"));
    assert_eq!(
        store.source_state(SCOPE, "unrelated-history").unwrap(),
        unrelated
    );
    assert!(store.health().unwrap().ok);
}

#[test]
fn failed_lifecycle_event_insert_rolls_back_generation_source_revision_and_claim_visibility() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = open(&temp);
    let captured = capture(&mut store, "events", QUOTE, true);
    learn(&mut store, "events", QUOTE);
    let before = store.source_state(SCOPE, "events").unwrap();
    sql(
        &store,
        "CREATE TRIGGER test_event_failure AFTER INSERT ON source_lifecycle_events BEGIN SELECT RAISE(ABORT,'synthetic event failure'); END;",
    );
    assert!(
        store
            .source_set_withdrawn(
                SCOPE,
                "events",
                text(&before, "revision_id"),
                before["generation"].as_i64().unwrap(),
                true,
                "Failed change"
            )
            .is_err()
    );
    assert!(
        store
            .source_import(
                SCOPE,
                source_input("events", "Changed source statement."),
                Some(text(&captured, "revision_id")),
                true
            )
            .is_err()
    );
    assert!(
        store
            .source_import(SCOPE, source_input("new-event", QUOTE), None, true)
            .is_err()
    );
    assert_eq!(store.source_state(SCOPE, "events").unwrap(), before);
    assert_eq!(count(store.path(), "source_lifecycle_events"), 1);
    assert_eq!(count(store.path(), "source_revisions"), 1);
    assert_eq!(count(store.path(), "sources"), 1);
    assert_eq!(recall(&store), 1);
    assert!(store.health().unwrap().ok);
}

#[test]
fn lifecycle_history_is_immutable_and_bounds_the_inspection_response() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = open(&temp);
    capture(&mut store, "bounded-history", QUOTE, false);
    for transition in 0..100 {
        set_withdrawn(&mut store, "bounded-history", transition % 2 == 0);
    }
    let state = store.source_state(SCOPE, "bounded-history").unwrap();
    assert_eq!(state["generation"], 101);
    assert_eq!(state["history"].as_array().unwrap().len(), 100);
    assert_eq!(state["history_truncated"], true);
    assert_eq!(state["history"][0]["generation"], 101);
    assert_eq!(state["history"][99]["generation"], 2);
    assert_eq!(count(store.path(), "source_lifecycle_events"), 101);
    let connection = Connection::open(store.path()).unwrap();
    assert!(
        connection
            .execute(
                "UPDATE source_lifecycle_events SET reason='overwritten'",
                []
            )
            .is_err()
    );
    assert!(
        connection
            .execute("DELETE FROM source_lifecycle_events", [])
            .is_err()
    );
    assert!(store.health().unwrap().ok);
}

#[test]
fn a_later_chunk_match_returns_the_complete_source_statement_or_omits_it() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = open(&temp);
    let prefix = "刊物 source introduction.\n";
    let quote = format!(
        "{}\n{}\nThe canarymarkerzeta detail preserves a qualified SQLite claim. {}",
        "Background context remains qualified. ".repeat(80),
        "For further details see ".repeat(40),
        "Follow-up explanatory context. ".repeat(55)
    )
    .trim()
    .to_owned();
    assert!((3000..8192).contains(&quote.len()));
    let body = format!("{prefix}{quote}\nEnd of original source.");
    capture(&mut store, "later-chunk", &body, true);
    learn(&mut store, "later-chunk", &quote);
    let request = |max_chars| ContextRequest {
        query: "canarymarkerzeta".into(),
        mode: SearchMode::Lexical,
        limit: 1,
        scope: Some(SCOPE.into()),
        max_chars,
        evidence_per_memory: 2,
    };
    // Matching a later search chunk must not cut away the beginning or final
    // qualification of an accepted source statement in the answer packet.
    let large = store.assemble_context(&request(14000), None).unwrap();
    assert_eq!(large.references.len(), 1);
    let small_budget = 3500;
    let small = store
        .assemble_context(&request(small_budget), None)
        .unwrap();
    assert!(small.references.is_empty());
    assert!(small.used_chars <= small_budget);
    assert!(small.truncated);
    for packet in [&large] {
        let reference = &packet.references[0];
        assert_eq!(reference.start_byte, 0);
        assert_eq!(reference.content, quote);
        assert_eq!(
            &quote[reference.start_byte..reference.end_byte],
            reference.content
        );
        let context = reference.source_claim.as_ref().unwrap();
        assert!(!context.citations.is_empty());
        for citation in &context.citations {
            assert_eq!(citation.quote, reference.content);
            assert_eq!(
                &body[citation.start_byte..citation.end_byte],
                reference.content
            );
            assert_eq!(citation.start_byte, prefix.len() + reference.start_byte);
            assert_eq!(citation.end_byte, prefix.len() + reference.end_byte);
            assert_eq!(
                citation.start_line,
                1 + body[..citation.start_byte]
                    .bytes()
                    .filter(|&byte| byte == b'\n')
                    .count()
            );
            assert_eq!(
                citation.end_line,
                1 + body.as_bytes()[..citation.end_byte - 1]
                    .iter()
                    .filter(|&&byte| byte == b'\n')
                    .count()
            );
        }
    }
}

#[test]
fn failed_schema_ten_migration_rolls_back_backfill_and_preserves_schema_nine_sources() {
    let temp = tempfile::tempdir().unwrap();
    let mut fixture = open(&temp);
    let original = capture(&mut fixture, "migration-rollback", QUOTE, true);
    let record = fixture
        .source_get(SCOPE, "migration-rollback", None)
        .unwrap();
    let citation = fixture
        .source_cite(
            SCOPE,
            "migration-rollback",
            text(&original, "revision_id"),
            0,
            QUOTE.len(),
        )
        .unwrap();
    let database = temp.path().join("schema-nine-rollback.sqlite");
    schema_nine(&database, std::slice::from_ref(&record));
    let connection = Connection::open(&database).unwrap();
    // Migration 10 reaches this collision only after creating and backfilling
    // lifecycle state, its history, and the durable source-learning queue.
    connection
        .execute_batch("CREATE TABLE source_claims (synthetic_collision INTEGER);")
        .unwrap();
    drop(connection);
    assert!(Store::open(&database, 64).is_err());
    let connection = Connection::open(&database).unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        9
    );
    assert_eq!(count(&database, "schema_migrations"), 9);
    assert_eq!(count(&database, "source_revisions"), 1);
    for name in [
        "source_lifecycle",
        "source_lifecycle_events",
        "source_learning_jobs",
        "source_valid_claims",
        "source_valid_supports",
    ] {
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE name=?1",
                    [name],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0,
            "partially retained schema object {name}"
        );
    }
    let retained: (String, String, String) = connection
        .query_row(
            "SELECT revision_id,content,content_hash FROM source_revisions",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(retained.0, text(&record, "revision_id"));
    assert_eq!(retained.1, QUOTE);
    assert_eq!(retained.2, text(&record, "content_hash"));
    connection
        .execute_batch("DROP TABLE source_claims;")
        .unwrap();
    drop(connection);
    let recovered = Store::open(&database, 64).unwrap();
    assert_eq!(recovered.health().unwrap().schema_version, 20);
    assert_eq!(count(&database, "source_lifecycle"), 1);
    assert_eq!(count(&database, "source_lifecycle_events"), 1);
    assert_eq!(count(&database, "source_learning_jobs"), 1);
    assert_eq!(
        recovered
            .source_get(SCOPE, "migration-rollback", None)
            .unwrap(),
        record
    );
    assert_eq!(
        recovered
            .source_cite(
                SCOPE,
                "migration-rollback",
                text(&original, "revision_id"),
                0,
                QUOTE.len()
            )
            .unwrap(),
        citation
    );
    assert!(recovered.health().unwrap().ok);
}
