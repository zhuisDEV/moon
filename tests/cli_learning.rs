use assert_cmd::Command;
use moon::learning::{ApplyInput, PrepareRequest};
use moon::{
    ContextRequest, DistillAction, DistillInput, EvidenceInput, HashEmbedding, SearchMode,
    SearchRequest, Store,
};
use serde_json::{Value, json};

fn moon(home: &std::path::Path) -> Command {
    let mut command = Command::new(assert_cmd::cargo::cargo_bin!("moon"));
    command
        .env_remove("MOON_DATABASE")
        .env_remove("MOON_HOME")
        .env_remove("MOON_EMBEDDING_DIMENSIONS")
        .args([
            "--home",
            home.to_str().unwrap(),
            "--dimensions",
            "64",
            "--json",
        ]);
    command
}

fn store(temp: &tempfile::TempDir) -> Store {
    Store::open(temp.path().join("state/moon.sqlite"), 64).unwrap()
}

fn record(store: &mut Store, session: &str, scope: &str, content: &str, at: i64) {
    store
        .record_evidence(EvidenceInput {
            session_id: session.into(),
            scope: scope.into(),
            title: None,
            content: content.into(),
            completed_at_ms: at,
            metadata_json: "{}".into(),
        })
        .unwrap();
}

fn request(key: &str) -> PrepareRequest {
    PrepareRequest {
        run_key: key.into(),
        rule_version: "rules-a".into(),
        occurrence: None,
        review_session: None,
        limit: 32,
        max_chars: 64_000,
        lease_ms: 1_200_000,
        max_attempts: 3,
        before_ms: None,
        after_ms: None,
        preview: false,
    }
}

fn proposal(key: &str, session: &str, content: &str) -> DistillInput {
    DistillInput {
        canonical_key: key.into(),
        memory_kind: "fact".into(),
        scope: "global".into(),
        title: None,
        content: content.into(),
        importance: 0.5,
        confidence: 1.0,
        pinned: false,
        evidence_session_id: session.into(),
        evidence_quote: content.into(),
        supersedes: None,
        valid_until_ms: None,
    }
}

fn action(kind: &str, key: &str, session: &str, content: &str) -> Value {
    json!({"action":kind,"canonical_key":key,"kind":"fact","content":content,"importance":0.5,"confidence":1.0,"evidence":[{"session_id":session,"quote":content}]})
}

fn input(actions: Vec<Value>) -> ApplyInput {
    serde_json::from_value(json!({"actions":actions})).unwrap()
}

fn input_with_deferred(actions: Vec<Value>, deferred: Vec<String>) -> ApplyInput {
    serde_json::from_value(json!({"actions":actions,"deferred_evidence":deferred})).unwrap()
}

#[test]
fn model_execution_provenance_preserves_unknowns_and_rejects_false_verification() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = store(&temp);
    record(
        &mut store,
        "provenance",
        "global",
        "The rehearsal requires approval.",
        100,
    );
    let prepared = store.learning_prepare(&request("day:provenance")).unwrap();
    let run = prepared["run_id"].as_str().unwrap();
    let metadata = json!({
        "model":"openai/gpt-6.1-terra", "reasoning":"max", "prompt_hash":"ab".repeat(32),
        "execution":{
            "requested":{"provider":"openai","model":"gpt-6.1-terra","reasoning":"max"},
            "observed":{"provider":"openai","model":"gpt-6.1-terra","backend":"openclaw","reasoning":null,"fallback_attempts":null},
            "identity":"verified"
        }
    });
    for (pointer, invalid) in [
        ("/execution/requested/model", json!("another-model")),
        ("/execution/requested/reasoning", json!("low")),
        ("/execution/observed/provider", json!("different-provider")),
        ("/execution/observed/model", json!("different-model")),
        (
            "/execution/observed/backend",
            json!("private-provider-diagnostic"),
        ),
        ("/execution/observed/fallback_attempts", json!(1)),
        ("/execution/identity", json!("unknown")),
    ] {
        let mut bad = metadata.clone();
        *bad.pointer_mut(pointer).unwrap() = invalid;
        let input: ApplyInput =
            serde_json::from_value(json!({"actions":[],"metadata":bad})).unwrap();
        let error = store.learning_apply(run, input, false).unwrap_err();
        assert_eq!(error.to_string(), "invalid model execution metadata");
    }
    for (provider, model, identity) in [
        (Value::Null, Value::Null, "unknown"),
        (json!("openai"), Value::Null, "partial"),
        (Value::Null, json!("gpt-6.1-terra"), "partial"),
    ] {
        let mut partial = metadata.clone();
        partial["execution"]["observed"]["provider"] = provider;
        partial["execution"]["observed"]["model"] = model;
        partial["execution"]["identity"] = json!(identity);
        let input: ApplyInput =
            serde_json::from_value(json!({"actions":[],"metadata":partial})).unwrap();
        store.learning_apply(run, input, true).unwrap();
    }
    let input: ApplyInput =
        serde_json::from_value(json!({"actions":[],"metadata":metadata})).unwrap();
    let outcome = store.learning_apply(run, input.clone(), false).unwrap();
    assert_eq!(outcome["metadata"], metadata);
    assert!(outcome["metadata"]["execution"]["observed"]["reasoning"].is_null());
    assert!(outcome["metadata"]["execution"]["observed"]["fallback_attempts"].is_null());
    drop(store);
    let mut reopened = Store::open(temp.path().join("state/moon.sqlite"), 64).unwrap();
    assert_eq!(reopened.learning_apply(run, input, false).unwrap(), outcome);
    assert!(reopened.health().unwrap().ok);
}

#[test]
fn preview_is_read_only_and_scope_cutoff_is_frozen() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = store(&temp);
    record(
        &mut store,
        "first",
        "global",
        "Atlas uses the blue database.",
        100,
    );
    record(
        &mut store,
        "foreign",
        "private",
        "Private service uses SQLite.",
        150,
    );
    record(
        &mut store,
        "future",
        "global",
        "Atlas uses the green database.",
        300,
    );
    let mut req = request("day:one");
    req.before_ms = Some(200);
    req.preview = true;
    let preview = store.learning_prepare(&req).unwrap();
    assert_eq!(preview["status"], "preview");
    assert!(preview["run_id"].is_null());
    assert_eq!(preview["evidence"].as_array().unwrap().len(), 1);
    assert_eq!(preview["evidence"][0]["session_id"], "first");
    assert!(
        store.learning_status().unwrap()["runs"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let readonly = Store::open_existing(store.path(), 64).unwrap();
    assert_eq!(readonly.learning_status().unwrap()["pending_evidence"], 3);
    drop(readonly);
    req.preview = false;
    let prepared = store.learning_prepare(&req).unwrap();
    let run = prepared["run_id"].as_str().unwrap();
    store.learning_apply(run, input(vec![]), false).unwrap();
    assert_eq!(store.learning_prepare(&req).unwrap()["status"], "committed");
    req.run_key = "day:two".into();
    let next = store.learning_prepare(&req).unwrap();
    assert_eq!(next["scope"], "private");
    assert_eq!(next["evidence"].as_array().unwrap().len(), 1);
}

#[test]
fn dry_run_and_invalid_batch_leave_no_mutation_then_commit_once() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = store(&temp);
    record(
        &mut store,
        "one",
        "global",
        "Atlas uses SQLite. Atlas supports local search.",
        100,
    );
    let prepared = store.learning_prepare(&request("day")).unwrap();
    let run = prepared["run_id"].as_str().unwrap();
    let good = action("create", "atlas:database", "one", "Atlas uses SQLite.");
    let invalid = action(
        "create",
        "atlas:false",
        "one",
        "This phrase is absent from the evidence.",
    );
    assert!(
        store
            .learning_apply(run, input(vec![good.clone(), invalid]), false)
            .is_err()
    );
    assert_eq!(store.health().unwrap().active_memories, 0);
    assert_eq!(store.learning_status().unwrap()["processed_evidence"], 0);
    let dry = store
        .learning_apply(run, input(vec![good.clone()]), true)
        .unwrap();
    assert_eq!(dry["status"], "dry_run");
    assert_eq!(store.health().unwrap().active_memories, 0);
    assert_eq!(store.learning_status().unwrap()["processed_evidence"], 0);
    let committed = store
        .learning_apply(run, input(vec![good.clone()]), false)
        .unwrap();
    assert_eq!(committed["processed_evidence"], 1);
    assert_eq!(
        store.learning_apply(run, input(vec![good]), false).unwrap(),
        committed
    );
    assert_eq!(store.health().unwrap().active_memories, 1);
    let connection = rusqlite::Connection::open(store.path()).unwrap();
    let persisted: String = connection
        .query_row(
            "SELECT snapshot_json || outcome_json FROM learning_runs",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!persisted.contains("Atlas uses SQLite"));
    assert!(store.health().unwrap().ok);
}

#[test]
fn partial_apply_keeps_deferred_sources_pending_and_replays_after_restart() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = store(&temp);
    record(&mut store, "one", "global", "Atlas uses SQLite.", 100);
    record(
        &mut store,
        "two",
        "global",
        "Atlas supports local search.",
        200,
    );
    let prepared = store.learning_prepare(&request("day:partial")).unwrap();
    let run = prepared["run_id"].as_str().unwrap().to_owned();
    // The deferred turn also supports an accepted fact; another rejected
    // candidate from that turn must still be reconsidered on a later day.
    let batch = input_with_deferred(
        vec![
            action("create", "atlas:database", "one", "Atlas uses SQLite."),
            action(
                "create",
                "atlas:search",
                "two",
                "Atlas supports local search.",
            ),
        ],
        vec!["two".into()],
    );
    let committed = store.learning_apply(&run, batch.clone(), false).unwrap();
    assert_eq!(committed["action_count"], 2);
    assert_eq!(committed["selected_evidence"], 2);
    assert_eq!(committed["processed_evidence"], 1);
    assert_eq!(committed["deferred_evidence"], 1);
    assert_eq!(committed["deferred_session_ids"], json!(["two"]));
    assert_eq!(store.learning_status().unwrap()["pending_evidence"], 1);
    assert_eq!(store.health().unwrap().citations, 2);
    drop(store);

    let mut reopened = Store::open(temp.path().join("state/moon.sqlite"), 64).unwrap();
    assert_eq!(
        reopened.learning_apply(&run, batch, false).unwrap(),
        committed
    );
    assert_eq!(reopened.health().unwrap().active_memories, 2);
    assert_eq!(reopened.health().unwrap().citations, 2);
    let replay = reopened.learning_prepare(&request("day:partial")).unwrap();
    assert_eq!(replay["status"], "committed");
    assert_eq!(replay["deferred_evidence"], 1);
    let status = reopened.learning_status().unwrap();
    assert_eq!(status["processed_evidence"], 1);
    assert_eq!(status["runs"][0]["deferred_evidence"], 1);
    assert!(status["runs"][0].get("deferred_session_ids").is_none());

    let next = reopened.learning_prepare(&request("next-day")).unwrap();
    let selected = next["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["selected"] == true)
        .map(|item| item["session_id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(selected, vec!["two"]);
    assert!(reopened.health().unwrap().ok);
}

#[test]
fn invalid_deferred_sources_are_rejected_before_any_action_or_ledger_change() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = store(&temp);
    record(&mut store, "support", "global", "Atlas uses SQLite.", 100);
    store
        .distill_memory(proposal("atlas:database", "support", "Atlas uses SQLite."))
        .unwrap();
    let old = store.learning_prepare(&request("old-day")).unwrap();
    store
        .learning_apply(old["run_id"].as_str().unwrap(), input(vec![]), false)
        .unwrap();
    record(
        &mut store,
        "selected",
        "global",
        "Atlas uses SQLite. Atlas supports local search.",
        200,
    );
    let prepared = store.learning_prepare(&request("new-day")).unwrap();
    let run = prepared["run_id"].as_str().unwrap();
    assert!(
        prepared["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| { e["session_id"] == "support" && e["selected"] == false })
    );
    let before = store.health().unwrap();
    for deferred in [
        vec!["unknown".into()],
        vec!["support".into()],
        vec!["selected".into(), "selected".into()],
        (0..129).map(|i| format!("session-{i}")).collect(),
    ] {
        assert!(
            store
                .learning_apply(
                    run,
                    input_with_deferred(
                        vec![action(
                            "create",
                            "atlas:search",
                            "selected",
                            "Atlas supports local search."
                        )],
                        deferred,
                    ),
                    false,
                )
                .is_err()
        );
        let after = store.health().unwrap();
        assert!(after.ok);
        assert_eq!(after.documents, before.documents);
        assert_eq!(after.active_memories, before.active_memories);
        assert_eq!(after.citations, before.citations);
        let status = store.learning_status().unwrap();
        assert_eq!(status["pending_evidence"], 1);
        assert_eq!(status["processed_evidence"], 1);
        assert_eq!(status["runs"][0]["status"], "prepared");
        assert_eq!(status["runs"][0]["deferred_evidence"], 0);
    }
    assert!(
        serde_json::from_value::<ApplyInput>(json!({
            "actions":[],"deferred_evidence":[],"diagnostics":"must not be accepted"
        }))
        .is_err()
    );
}

#[test]
fn partial_apply_dry_run_and_invalid_actions_preserve_the_entire_transaction() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = store(&temp);
    record(&mut store, "one", "global", "Atlas uses SQLite.", 100);
    record(&mut store, "two", "global", "Atlas supports 20 tools.", 200);
    let prepared = store.learning_prepare(&request("day:dry-partial")).unwrap();
    let run = prepared["run_id"].as_str().unwrap();
    let good = action("create", "atlas:database", "one", "Atlas uses SQLite.");
    let batch = input_with_deferred(vec![good.clone()], vec!["two".into()]);
    let dry = store.learning_apply(run, batch.clone(), true).unwrap();
    assert_eq!(dry["status"], "dry_run");
    assert_eq!(dry["action_count"], 1);
    assert_eq!(dry["selected_evidence"], 2);
    assert_eq!(dry["processed_evidence"], 0);
    assert_eq!(dry["deferred_evidence"], 1);
    assert_eq!(dry["deferred_session_ids"], json!(["two"]));
    let mut invalid = action("create", "atlas:tools", "two", "Atlas supports 200 tools.");
    invalid["evidence"][0]["quote"] = "Atlas supports 20 tools.".into();
    assert!(
        store
            .learning_apply(
                run,
                input_with_deferred(vec![good, invalid], vec!["two".into()]),
                false,
            )
            .is_err()
    );
    let health = store.health().unwrap();
    assert!(health.ok);
    assert_eq!(health.active_memories, 0);
    assert_eq!(health.citations, 0);
    assert_eq!(health.pending_embeddings, 0);
    let status = store.learning_status().unwrap();
    assert_eq!(status["pending_evidence"], 2);
    assert_eq!(status["processed_evidence"], 0);
    assert_eq!(status["runs"][0]["status"], "prepared");
    assert_eq!(status["runs"][0]["deferred_evidence"], 0);
    let committed = store.learning_apply(run, batch, false).unwrap();
    assert_eq!(committed["processed_evidence"], 1);
    assert_eq!(committed["deferred_evidence"], 1);
    assert!(store.health().unwrap().ok);
}

#[test]
fn older_committed_outcomes_default_to_zero_deferred_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = store(&temp);
    record(&mut store, "one", "global", "Atlas uses SQLite.", 100);
    let prepared = store.learning_prepare(&request("legacy-day")).unwrap();
    let run = prepared["run_id"].as_str().unwrap();
    let outcome = store.learning_apply(run, input(vec![]), false).unwrap();
    assert_eq!(outcome["deferred_evidence"], 0);
    assert_eq!(outcome["deferred_session_ids"], json!([]));
    let connection = rusqlite::Connection::open(store.path()).unwrap();
    connection.execute(
        "UPDATE learning_runs SET outcome_json=json_remove(outcome_json,'$.deferred_evidence','$.deferred_session_ids') WHERE run_id=?1",
        [run],
    ).unwrap();
    drop(connection);
    drop(store);
    let mut reopened = Store::open(temp.path().join("state/moon.sqlite"), 64).unwrap();
    let replay = reopened.learning_prepare(&request("legacy-day")).unwrap();
    assert_eq!(replay["status"], "committed");
    assert_eq!(replay["deferred_evidence"], 0);
    let status = reopened.learning_status().unwrap();
    assert_eq!(status["runs"][0]["deferred_evidence"], 0);
    assert_eq!(status["pending_evidence"], 0);
    assert!(reopened.health().unwrap().ok);
}

#[test]
fn expired_and_failed_leases_recover_after_restart() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = store(&temp);
    record(&mut store, "one", "global", "Atlas uses SQLite.", 100);
    let first = store.learning_prepare(&request("day")).unwrap();
    assert_eq!(
        store.learning_prepare(&request("other")).unwrap()["status"],
        "busy"
    );
    let first_id = first["run_id"].as_str().unwrap().to_string();
    let path = store.path().to_path_buf();
    drop(store);
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute("UPDATE learning_runs SET lease_until_ms=1", [])
        .unwrap();
    drop(connection);
    let mut store = Store::open(&path, 64).unwrap();
    let second = store.learning_prepare(&request("day")).unwrap();
    assert_ne!(second["run_id"], first_id);
    assert!(
        store
            .learning_apply(&first_id, input(vec![]), false)
            .is_err()
    );
    store
        .learning_fail(second["run_id"].as_str().unwrap())
        .unwrap();
    let third = store.learning_prepare(&request("day")).unwrap();
    assert_eq!(third["evidence"][0]["session_id"], "one");
    assert_eq!(store.learning_status().unwrap()["processed_evidence"], 0);
}

#[test]
fn l1_change_invalidates_l2_snapshot_and_cross_scope_quotes_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = store(&temp);
    record(&mut store, "one", "global", "Atlas uses SQLite.", 100);
    record(
        &mut store,
        "foreign",
        "private",
        "Private database uses PostgreSQL.",
        200,
    );
    let first = store.learning_prepare(&request("day")).unwrap();
    let run = first["run_id"].as_str().unwrap();
    assert!(
        store
            .learning_apply(
                run,
                input(vec![action(
                    "create",
                    "leaked:key",
                    "foreign",
                    "Private database uses PostgreSQL."
                )]),
                false
            )
            .is_err()
    );
    store
        .distill_memory(proposal("atlas:database", "one", "Atlas uses SQLite."))
        .unwrap();
    assert!(
        store
            .learning_apply(run, input(vec![]), false)
            .unwrap_err()
            .to_string()
            .contains("memory changed")
    );
    store.learning_fail(run).unwrap();
    assert_eq!(store.learning_status().unwrap()["processed_evidence"], 0);
}

#[test]
fn concurrent_correction_leaves_all_members_of_a_stale_l2_batch_unapplied() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = store(&temp);
    let seed = "The blue notebook label is Topaz.";
    let correction = "The blue is blank. The green notebook has the Topaz label.";
    record(&mut store, "seed", "global", seed, 100);
    let old = store
        .distill_memory(proposal("blue:label", "seed", seed))
        .unwrap();
    record(&mut store, "correction", "global", correction, 200);
    let packet = store.learning_prepare(&request("concurrent")).unwrap();
    let run = packet["run_id"].as_str().unwrap();
    let mut blue = action(
        "supersede",
        "blue:label",
        "correction",
        "The blue is blank.",
    );
    blue["target_document_id"] = old.document_id.into();
    let batch = input(vec![
        action(
            "create",
            "green:label",
            "correction",
            "The green notebook has the Topaz label.",
        ),
        blue,
    ]);

    let latest = "The blue notebook now has a Pine label.";
    record(&mut store, "latest", "global", latest, 300);
    let mut concurrent = proposal("blue:label", "latest", latest);
    concurrent.supersedes = Some(old.document_id);
    let newest = store.distill_memory(concurrent).unwrap();
    assert!(
        store
            .learning_apply(run, batch, false)
            .unwrap_err()
            .to_string()
            .contains("memory changed")
    );
    assert_eq!(store.health().unwrap().active_memories, 1);
    let db = rusqlite::Connection::open(store.path()).unwrap();
    let head: i64 = db
        .query_row(
            "SELECT document_id FROM memory_items WHERE superseded_by IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(head, newest.document_id);
    assert_eq!(store.learning_status().unwrap()["processed_evidence"], 0);
}

#[test]
fn exact_duplicates_get_aliases_and_corrections_require_newer_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = store(&temp);
    record(&mut store, "one", "global", "Atlas uses SQLite.", 100);
    record(&mut store, "old", "global", "Atlas uses PostgreSQL.", 50);
    record(&mut store, "new", "global", "Atlas uses PostgreSQL.", 200);
    let first = store
        .distill_memory(proposal("atlas:database", "one", "Atlas uses SQLite."))
        .unwrap();
    let duplicate = store
        .distill_memory(proposal(
            "atlas:database:other",
            "one",
            "Atlas uses SQLite.",
        ))
        .unwrap();
    assert_eq!(duplicate.document_id, first.document_id);
    assert_eq!(duplicate.action, DistillAction::Confirmed);
    assert_eq!(duplicate.canonical_key, "atlas:database");
    let mut older = proposal("atlas:database:other", "old", "Atlas uses PostgreSQL.");
    older.supersedes = Some(first.document_id);
    assert!(store.distill_memory(older).is_err());
    let mut newer = proposal("atlas:database:other", "new", "Atlas uses PostgreSQL.");
    newer.supersedes = Some(first.document_id);
    let second = store.distill_memory(newer).unwrap();
    assert_eq!(second.action, DistillAction::Superseded);
    let connection = rusqlite::Connection::open(store.path()).unwrap();
    let alias: i64 = connection
        .query_row(
            "SELECT document_id FROM memory_aliases WHERE canonical_key='atlas:database:other'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(alias, second.document_id);
    let confirmed: i64 = connection
        .query_row(
            "SELECT last_confirmed_at_ms FROM memory_items WHERE document_id=?1",
            [second.document_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(confirmed, 200);
    assert!(store.health().unwrap().ok);
}

#[test]
fn expired_observations_are_not_retrieved_and_confirmation_keeps_expiry() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = store(&temp);
    record(
        &mut store,
        "one",
        "global",
        "Atlas service is responding.",
        100,
    );
    let mut observation = proposal("atlas:status", "one", "Atlas service is responding.");
    observation.valid_until_ms = Some(200);
    store.distill_memory(observation).unwrap();
    store
        .distill_memory(proposal(
            "atlas:status",
            "one",
            "Atlas service is responding.",
        ))
        .unwrap();
    let packet = store
        .assemble_context(
            &ContextRequest {
                query: "Atlas service".into(),
                mode: SearchMode::Lexical,
                limit: 8,
                scope: Some("global".into()),
                max_chars: 3500,
                evidence_per_memory: 2,
            },
            None,
        )
        .unwrap();
    assert!(packet.memories.is_empty());
    let related = store
        .learning_related("global", "Atlas service", 8, 16000)
        .unwrap();
    assert_eq!(related["memories"][0]["observed_at_ms"], 100);
    assert_eq!(related["memories"][0]["valid_until_ms"], 200);
    let health = store.health().unwrap();
    assert!(health.ok);
    assert_eq!(health.pending_embeddings, 0);
    assert_eq!(
        store
            .embed_pending(&HashEmbedding::new(64), 10)
            .unwrap()
            .selected,
        0
    );
}

#[test]
fn l2_historical_observations_commit_without_index_corruption_or_embedding_work() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = store(&temp);
    let content = "Atlas service is responding.";
    record(&mut store, "historical", "global", content, 100);
    let prepared = store.learning_prepare(&request("historical:day")).unwrap();
    let mut value = action("create", "atlas:status", "historical", content);
    value["kind"] = "observation".into();
    value["valid_until_ms"] = 200.into();
    let outcome = store
        .learning_apply(
            prepared["run_id"].as_str().unwrap(),
            input(vec![value]),
            false,
        )
        .unwrap();
    assert_eq!(outcome["processed_evidence"], 1);
    let health = store.health().unwrap();
    assert!(health.ok);
    assert_eq!(health.active_memories, 0);
    assert_eq!(health.citations, 1);
    assert_eq!(health.pending_embeddings, 0);
    let report = store.embed_pending(&HashEmbedding::new(64), 10).unwrap();
    assert_eq!(report.selected, 0);
    assert_eq!(report.embedded, 0);
    assert_eq!(report.remaining, 0);
    let hits = store
        .search(
            &SearchRequest {
                query: "Atlas service".into(),
                mode: SearchMode::Lexical,
                limit: 1,
                scope: Some("global".into()),
                source_kind: Some("memory".into()),
            },
            None,
        )
        .unwrap();
    assert!(hits.is_empty());
    assert!(
        Store::open_existing(store.path(), 64)
            .unwrap()
            .health()
            .unwrap()
            .ok
    );
}

#[test]
fn fresh_confirmation_restores_expired_indexes_after_maintenance() {
    for keep_cached_vector in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let mut store = store(&temp);
        let content = "Atlas service is responding.";
        record(&mut store, "old", "global", content, 100);
        let mut value = proposal("atlas:status", "old", content);
        value.valid_until_ms = Some(chrono::Utc::now().timestamp_millis() + 86_400_000);
        let original = store.distill_memory(value).unwrap();
        let provider = HashEmbedding::new(64);
        assert_eq!(store.embed_pending(&provider, 10).unwrap().embedded, 1);
        // Move the fixture past its deadline without invoking any maintenance
        // path, leaving the same cached indexes as natural wall-clock expiry.
        let connection = rusqlite::Connection::open(store.path()).unwrap();
        connection
            .execute(
                "UPDATE memory_items SET valid_until_ms=200 WHERE document_id=?1",
                [original.document_id],
            )
            .unwrap();
        let expired = Store::open_existing(store.path(), 64)
            .unwrap()
            .health()
            .unwrap();
        assert!(expired.ok);
        assert_eq!(expired.active_memories, 0);
        assert_eq!(expired.vectors, 1);
        store.rebuild_fts().unwrap();
        if !keep_cached_vector {
            assert_eq!(store.requeue_embeddings().unwrap(), 0);
        }
        assert!(store.health().unwrap().ok);

        record(&mut store, "fresh", "global", content, 300);
        let mut confirmation = proposal("atlas:status", "fresh", content);
        confirmation.valid_until_ms = Some(chrono::Utc::now().timestamp_millis() + 86_400_000);
        let outcome = store.distill_memory(confirmation).unwrap();
        assert_eq!(outcome.document_id, original.document_id);
        assert_eq!(outcome.action, DistillAction::Confirmed);
        assert_eq!(outcome.evidence_count, 2);
        let health = store.health().unwrap();
        assert!(health.ok);
        assert_eq!(health.active_memories, 1);
        assert_eq!(health.pending_embeddings, usize::from(!keep_cached_vector));
        assert_eq!(
            store.embed_pending(&provider, 10).unwrap().embedded,
            usize::from(!keep_cached_vector)
        );
        for mode in [SearchMode::Lexical, SearchMode::Semantic] {
            let hits = store
                .search(
                    &SearchRequest {
                        query: "Atlas service".into(),
                        mode,
                        limit: 1,
                        scope: Some("global".into()),
                        source_kind: Some("memory".into()),
                    },
                    Some(&provider),
                )
                .unwrap();
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].document_id, original.document_id);
        }
        assert!(store.health().unwrap().ok);
    }
}

#[test]
fn l2_accepts_source_faithful_paraphrases_without_matching_qualifier_words() {
    for (quote, content) in [
        (
            "No Atlas backups are stored.",
            "Atlas has zero stored backups.",
        ),
        (
            "If Atlas uses SQLite, local search is possible.",
            "Using SQLite would enable local search for Atlas.",
        ),
        (
            "Atlas previously used Live Room.",
            "Live Room saw use in an earlier Atlas setup.",
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let mut store = store(&temp);
        record(&mut store, "one", "global", quote, 100);
        let prepared = store.learning_prepare(&request("day")).unwrap();
        let mut value = action("create", "atlas:claim", "one", content);
        value["evidence"][0]["quote"] = quote.into();
        let result = store
            .learning_apply(
                prepared["run_id"].as_str().unwrap(),
                input(vec![value]),
                false,
            )
            .unwrap();
        assert_eq!(result["action_count"], 1);
    }
}

#[test]
fn oversized_oldest_evidence_is_visible_and_not_skipped() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = store(&temp);
    record(
        &mut store,
        "large",
        "global",
        &"Large evidence record. ".repeat(100),
        100,
    );
    record(&mut store, "small", "global", "Atlas uses SQLite.", 200);
    let mut req = request("day");
    req.max_chars = 1024;
    assert!(
        store
            .learning_prepare(&req)
            .unwrap_err()
            .to_string()
            .contains("oldest pending evidence")
    );
    assert_eq!(store.learning_status().unwrap()["pending_evidence"], 2);
    assert!(
        store.learning_status().unwrap()["runs"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn review_is_visible_without_changing_claims() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = store(&temp);
    record(&mut store, "one", "global", "Atlas might use SQLite.", 100);
    let prepared = store.learning_prepare(&request("day")).unwrap();
    store
        .learning_apply(
            prepared["run_id"].as_str().unwrap(),
            input(vec![action(
                "review",
                "atlas:database",
                "one",
                "Atlas might use SQLite.",
            )]),
            false,
        )
        .unwrap();
    assert_eq!(store.health().unwrap().active_memories, 0);
    let status = store.learning_status().unwrap();
    assert_eq!(status["reviews"][0]["canonical_key"], "atlas:database");
    assert_eq!(status["processed_evidence"], 1);
}

#[test]
fn cli_round_trip_and_read_only_commands_never_create_a_home() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("moon");
    moon(&home).args(["learning", "status"]).assert().failure();
    assert!(!home.exists());
    moon(&home)
        .args([
            "record",
            "--session-id",
            "one",
            "--content",
            "Atlas uses SQLite.",
            "--completed-at-ms",
            "100",
        ])
        .assert()
        .success();
    moon(&home)
        .args(["learning", "prepare", "--run-key", "preview", "--preview"])
        .assert()
        .success();
    let output = moon(&home)
        .args([
            "learning",
            "prepare",
            "--run-key",
            "day",
            "--before-ms",
            "200",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let prepared: Value = serde_json::from_slice(&output.stdout).unwrap();
    let payload = json!({"actions":[action("create","atlas:database","one","Atlas uses SQLite.")]})
        .to_string();
    moon(&home)
        .args([
            "learning",
            "apply",
            "--run-id",
            prepared["run_id"].as_str().unwrap(),
            "--input",
            "-",
        ])
        .write_stdin(payload)
        .assert()
        .success();
    let output = moon(&home)
        .args([
            "learning",
            "related",
            "--query",
            "Atlas database",
            "--scope",
            "global",
        ])
        .output()
        .unwrap();
    let related: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(related["memories"][0]["canonical_key"], "atlas:database");
    moon(&home).args(["health"]).assert().success();
}

#[test]
fn legacy_identical_heads_merge_with_citations_and_aliases_preserved() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = store(&temp);
    record(&mut store, "one", "global", "Atlas uses SQLite.", 100);
    let first = store
        .distill_memory(proposal("atlas:database", "one", "Atlas uses SQLite."))
        .unwrap();
    // Seed the duplicate shape that older releases could create. Current L1
    // intentionally cannot create this fixture through its public API.
    let connection = rusqlite::Connection::open(store.path()).unwrap();
    connection.execute(
        "INSERT INTO documents(source_uri,source_kind,scope,title,content_hash,modified_at_ms,indexed_at_ms,active,metadata_json,body)
         SELECT source_uri||'/legacy-duplicate',source_kind,scope,title,content_hash,modified_at_ms,indexed_at_ms,active,metadata_json,body FROM documents WHERE id=?1",
        [first.document_id],
    ).unwrap();
    let duplicate = connection.last_insert_rowid();
    connection.execute(
        "INSERT INTO memory_items(document_id,memory_kind,importance,confidence,valid_from_ms,pinned,canonical_key,last_confirmed_at_ms,observed_at_ms)
         SELECT ?2,memory_kind,importance,confidence,valid_from_ms,pinned,'atlas:legacy',last_confirmed_at_ms,observed_at_ms FROM memory_items WHERE document_id=?1",
        rusqlite::params![first.document_id,duplicate],
    ).unwrap();
    connection.execute("INSERT INTO memory_heads(scope,canonical_key,document_id,updated_at_ms) VALUES('global','atlas:legacy',?1,100)",[duplicate]).unwrap();
    connection.execute(
        "INSERT INTO memory_citations(memory_document_id,evidence_session_id,evidence_document_id,start_byte,end_byte,start_line,end_line,quote,created_at_ms)
         SELECT ?2,evidence_session_id,evidence_document_id,start_byte,end_byte,start_line,end_line,quote,created_at_ms FROM memory_citations WHERE memory_document_id=?1",
        rusqlite::params![first.document_id,duplicate],
    ).unwrap();
    drop(connection);
    let reviewed = store.learning_prepare(&request("legacy-review")).unwrap();
    let mut review = action("review", "atlas:legacy", "one", "Atlas uses SQLite.");
    review["target_document_id"] = duplicate.into();
    store
        .learning_apply(
            reviewed["run_id"].as_str().unwrap(),
            input(vec![review]),
            false,
        )
        .unwrap();
    record(&mut store, "newer", "global", "Atlas uses SQLite.", 300);
    let duplicate_expiry = chrono::Utc::now().timestamp_millis() + 172_800_000;
    let mut confirmation = proposal("atlas:legacy", "newer", "Atlas uses SQLite.");
    confirmation.valid_until_ms = Some(duplicate_expiry);
    confirmation.pinned = true;
    confirmation.importance = 0.9;
    store.distill_memory(confirmation).unwrap();
    record(&mut store, "two", "global", "Atlas uses SQLite.", 200);
    let prepared = store.learning_prepare(&request("merge-day")).unwrap();
    let mut merge = action("merge", "atlas:database", "two", "Atlas uses SQLite.");
    merge["target_document_id"] = first.document_id.into();
    merge["merge_document_ids"] = json!([duplicate]);
    store
        .learning_apply(
            prepared["run_id"].as_str().unwrap(),
            input(vec![merge]),
            false,
        )
        .unwrap();
    assert_eq!(store.health().unwrap().active_memories, 1);
    assert!(store.health().unwrap().ok);
    let connection = rusqlite::Connection::open(store.path()).unwrap();
    let alias: i64 = connection
        .query_row(
            "SELECT document_id FROM memory_aliases WHERE canonical_key='atlas:legacy'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(alias, first.document_id);
    let retired: i64 = connection
        .query_row(
            "SELECT superseded_by FROM memory_items WHERE document_id=?1",
            [duplicate],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retired, first.document_id);
    let citations: i64 = connection
        .query_row(
            "SELECT count(*) FROM memory_citations WHERE memory_document_id=?1",
            [first.document_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(citations, 3);
    let (confirmed,expiry,pinned,importance):(i64,i64,bool,f64)=connection.query_row(
        "SELECT last_confirmed_at_ms,valid_until_ms,pinned,importance FROM memory_items WHERE document_id=?1",
        [first.document_id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
    ).unwrap();
    assert_eq!(confirmed, 300);
    assert_eq!(expiry, duplicate_expiry);
    assert!(pinned);
    assert_eq!(importance, 0.9);
    let related = store
        .learning_related("global", "Atlas SQLite", 8, 16000)
        .unwrap();
    assert_eq!(related["memories"][0]["review_required"], true);
    assert!(
        store.learning_status().unwrap()["reviews"]
            .as_array()
            .unwrap()
            .iter()
            .any(|review| review["document_id"] == first.document_id)
    );
}

#[test]
fn superseding_an_expired_observation_does_not_extend_its_historical_validity() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = store(&temp);
    record(
        &mut store,
        "old",
        "global",
        "Atlas service is responding.",
        100,
    );
    record(
        &mut store,
        "new",
        "global",
        "Atlas service is unavailable.",
        300,
    );
    let mut initial = proposal("atlas:status", "old", "Atlas service is responding.");
    initial.valid_until_ms = Some(200);
    let first = store.distill_memory(initial).unwrap();
    let mut replacement = proposal("atlas:status", "new", "Atlas service is unavailable.");
    replacement.supersedes = Some(first.document_id);
    replacement.valid_until_ms = Some(400);
    store.distill_memory(replacement).unwrap();
    let connection = rusqlite::Connection::open(store.path()).unwrap();
    let expiry: i64 = connection
        .query_row(
            "SELECT valid_until_ms FROM memory_items WHERE document_id=?1",
            [first.document_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(expiry, 200);
}

#[test]
fn l1_scopes_keys_and_rejects_cross_scope_evidence_and_kind_collisions() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = store(&temp);
    record(&mut store, "one", "global", "Atlas uses SQLite.", 100);
    record(&mut store, "private", "private", "Atlas uses SQLite.", 200);
    store
        .distill_memory(proposal("atlas:database", "one", "Atlas uses SQLite."))
        .unwrap();
    let mut cross = proposal("atlas:private", "private", "Atlas uses SQLite.");
    assert!(store.distill_memory(cross.clone()).is_err());
    cross.scope = "private".into();
    cross.canonical_key = "atlas:database".into();
    store
        .distill_memory(cross)
        .expect("the same key in a different scope is independent");
    let mut kind = proposal("atlas:database", "one", "Atlas uses SQLite.");
    kind.memory_kind = "preference".into();
    assert!(store.distill_memory(kind).is_err());
    assert_eq!(store.health().unwrap().active_memories, 2);
}

#[test]
fn failed_attempt_budget_survives_restart_and_expired_leases_count() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = store(&temp);
    record(&mut store, "one", "global", "Atlas uses SQLite.", 100);
    for attempt in 0..3 {
        let prepared = store.learning_prepare(&request("daily:batch0")).unwrap();
        assert_eq!(prepared["status"], "prepared");
        if attempt == 1 {
            let connection = rusqlite::Connection::open(store.path()).unwrap();
            connection
                .execute(
                    "UPDATE learning_runs SET lease_until_ms=1 WHERE status='prepared'",
                    [],
                )
                .unwrap();
        } else {
            store
                .learning_fail(prepared["run_id"].as_str().unwrap())
                .unwrap();
        }
    }
    let path = store.path().to_path_buf();
    drop(store);
    let mut store = Store::open(path, 64).unwrap();
    let exhausted = store.learning_prepare(&request("daily:batch0")).unwrap();
    assert_eq!(exhausted["status"], "exhausted");
    assert_eq!(exhausted["failed_attempts"], 3);
    assert_eq!(store.learning_status().unwrap()["processed_evidence"], 0);
    assert_eq!(
        store.learning_prepare(&request("tomorrow:batch0")).unwrap()["status"],
        "prepared"
    );
}

#[test]
fn prepare_reserves_room_for_original_context_and_reports_omissions() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = store(&temp);
    let old = format!(
        "Atlas uses SQLite. {}",
        "Original context detail. ".repeat(60)
    );
    record(&mut store, "old", "global", &old, 100);
    store
        .distill_memory(proposal("atlas:database", "old", "Atlas uses SQLite."))
        .unwrap();
    let first = store.learning_prepare(&request("old-processed")).unwrap();
    store
        .learning_apply(first["run_id"].as_str().unwrap(), input(vec![]), false)
        .unwrap();
    for n in 0..5 {
        record(
            &mut store,
            &format!("new-{n}"),
            "global",
            &format!(
                "Atlas uses SQLite. {}",
                "New supporting context. ".repeat(40)
            ),
            200 + n,
        );
    }
    let mut req = request("today");
    req.max_chars = 6000;
    let prepared = store.learning_prepare(&req).unwrap();
    assert!(!prepared["memories"].as_array().unwrap().is_empty());
    let evidence = prepared["evidence"].as_array().unwrap();
    assert!(
        evidence
            .iter()
            .any(|e| e["session_id"] == "old" && e["selected"] == false)
    );
    assert!(evidence.iter().filter(|e| e["selected"] == true).count() < 5);
    assert!(serde_json::to_string(&prepared).unwrap().chars().count() <= req.max_chars);
    store
        .learning_fail(prepared["run_id"].as_str().unwrap())
        .unwrap();
    req.max_chars = 1800;
    assert!(
        store
            .learning_prepare(&req)
            .unwrap_err()
            .to_string()
            .contains("related memory and original evidence")
    );
    req.max_chars = 2_097_152;
    assert_eq!(store.learning_prepare(&req).unwrap()["status"], "prepared");
}

#[test]
fn prepare_reports_known_omissions_and_unscanned_candidates_separately() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = store(&temp);
    let mut req = request("bounded-comparison");
    req.preview = true;
    req.max_chars = 2_097_152;
    req.after_ms = Some(200);
    record(
        &mut store,
        "new",
        "global",
        "Cedar reconciliation verifies checklist entries.",
        200,
    );
    let mut first = 0;
    for (count, omitted, scan_limited) in [
        (128, 0, false),
        (130, 2, false),
        (512, 384, false),
        (513, 384, true),
    ] {
        let lines: Vec<String> = (first..count)
            .map(|n| format!("Cedar reconciliation verifies checklist entry {n}."))
            .collect();
        let session = format!("old-{count}");
        record(&mut store, &session, "global", &lines.join("\n"), 100);
        let claims: Vec<_> = lines
            .iter()
            .enumerate()
            .map(|(i, line)| proposal(&format!("cedar:{}", first + i), &session, line))
            .collect();
        for batch in claims.chunks(32) {
            store.distill_batch(batch.to_vec()).unwrap();
        }
        let packet = store.learning_prepare(&req).unwrap();
        assert_eq!(packet["memories"].as_array().unwrap().len(), 128);
        assert_eq!(packet["omitted_memory_count"], omitted);
        assert_eq!(packet["candidate_scan_limited"], scan_limited);
        assert_eq!(packet["context_limited"], omitted > 0 || scan_limited);
        first = count;
    }
    // Newer candidates may all be irrelevant. The scan cap still prevents
    // a complete comparison even when no scored match was omitted.
    record(
        &mut store,
        "unrelated",
        "global",
        "Packed vinegar bottles.",
        300,
    );
    req.after_ms = Some(300);
    let packet = store.learning_prepare(&req).unwrap();
    assert!(packet["memories"].as_array().unwrap().is_empty());
    assert_eq!(packet["omitted_memory_count"], 0);
    assert_eq!(packet["candidate_scan_limited"], true);
    assert_eq!(packet["context_limited"], true);
}

#[test]
fn numeric_claims_preserve_complete_tokens_and_normalize_only_grouped_commas() {
    for (source, changed) in [
        ("100", "-100"),
        ("-100", "100"),
        ("0.5", "5"),
        (".5", "5"),
        ("2.5", "2.5.3"),
        ("2.5.3", "2.5"),
        ("12:30", "12:00"),
        ("1e3", "1e4"),
        ("9007199254740992", "9007199254740993"),
        ("10,00", "1000"),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let mut store = store(&temp);
        let original = format!("Atlas configured value is {source}.");
        let wrong = format!("Atlas configured value is {changed}.");
        record(&mut store, "one", "global", &original, 100);
        let prepared = store.learning_prepare(&request("day")).unwrap();
        let mut value = action("create", "atlas:number", "one", &wrong);
        value["evidence"][0]["quote"] = original.into();
        assert!(
            store
                .learning_apply(
                    prepared["run_id"].as_str().unwrap(),
                    input(vec![value]),
                    false
                )
                .is_err(),
            "{source} -> {changed}"
        );
    }
    let temp = tempfile::tempdir().unwrap();
    let mut store = store(&temp);
    record(
        &mut store,
        "one",
        "global",
        "Atlas has 10,000 records.",
        100,
    );
    let prepared = store.learning_prepare(&request("day")).unwrap();
    let mut value = action("create", "atlas:number", "one", "Atlas has 10000 records.");
    value["evidence"][0]["quote"] = "Atlas has 10,000 records.".into();
    store
        .learning_apply(
            prepared["run_id"].as_str().unwrap(),
            input(vec![value]),
            false,
        )
        .unwrap();
}

#[test]
fn l2_transfers_a_notebook_label_across_turns_as_one_atomic_correction() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = store(&temp);
    let original = "The blue Fieldwork notebook has cover label Topaz 42.";
    let correction = "I meant the green Fieldwork notebook. The blue is blank.";
    record(&mut store, "seed", "global", original, 100);
    let blue = store
        .distill_memory(proposal("fieldwork:blue", "seed", original))
        .unwrap();
    // The earlier source may already be processed. L2 brings it back through the old head.
    let prepared = store.learning_prepare(&request("seed")).unwrap();
    store
        .learning_apply(prepared["run_id"].as_str().unwrap(), input(vec![]), false)
        .unwrap();
    record(&mut store, "correction", "global", correction, 200);
    let prepared = store
        .learning_prepare(&request("reconcile-notebooks"))
        .unwrap();
    let run = prepared["run_id"].as_str().unwrap();
    assert!(
        prepared["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["session_id"] == "seed" && item["selected"] == false)
    );
    let mut blank = action(
        "supersede",
        "fieldwork:blue",
        "correction",
        "The blue is blank.",
    );
    blank["target_document_id"] = blue.document_id.into();
    let mut green = action(
        "create",
        "fieldwork:green",
        "seed",
        "The green Fieldwork notebook has cover label Topaz 42.",
    );
    green["evidence"] = json!([{"session_id":"seed","quote":original},{"session_id":"correction","quote":correction}]);
    let mut bad_green = green.clone();
    bad_green["evidence"][1]["quote"] = "I meant the other Fieldwork notebook.".into();
    assert!(
        store
            .learning_apply(run, input(vec![blank.clone(), bad_green]), false)
            .unwrap_err()
            .to_string()
            .contains("not found exactly")
    );
    let db = rusqlite::Connection::open(store.path()).unwrap();
    let heads = || -> Vec<String> {
        db.prepare("SELECT d.body FROM documents d JOIN memory_items m ON m.document_id=d.id WHERE d.active AND m.superseded_by IS NULL ORDER BY m.canonical_key").unwrap()
            .query_map([], |row| row.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap()
    };
    assert_eq!(heads(), [original]);
    assert_eq!(store.learning_status().unwrap()["pending_evidence"], 1);
    let batch = input(vec![blank, green]);
    let committed = store.learning_apply(run, batch.clone(), false).unwrap();
    assert_eq!(committed["action_count"], 2);
    assert_eq!(
        heads(),
        [
            "The blue is blank.",
            "The green Fieldwork notebook has cover label Topaz 42."
        ]
    );
    assert_eq!(store.learning_apply(run, batch, false).unwrap(), committed);
    let citations: i64 = db
        .query_row(
            "SELECT count(*) FROM memory_citations WHERE memory_document_id=?1",
            [committed["outcomes"][1]["document_id"].as_i64().unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(citations, 2);

    let ambiguous = "Fieldwork notebook correction: I meant the other one.";
    record(&mut store, "ambiguous", "global", ambiguous, 300);
    let prepared = store.learning_prepare(&request("ambiguous")).unwrap();
    let run = prepared["run_id"].as_str().unwrap();
    let before = heads();
    let deferred = store
        .learning_apply(
            run,
            input_with_deferred(vec![], vec!["ambiguous".into()]),
            false,
        )
        .unwrap();
    assert_eq!(deferred["deferred_evidence"], 1);
    assert_eq!(heads(), before);
}

#[test]
fn l1_and_l2_accept_short_complete_speaker_replies_and_reject_fragments() {
    for (body, quote, valid) in [
        ("Green.", "Green.", true),
        ("User:\n是绿色的。", "是绿色的。", true),
        ("User:\n7", "7", true),
        ("User:\nGreen.", "Green", false),
        ("User:\nGreen.", "User:\nGreen.", false),
        ("User:\nGreen.", "User:", false),
        ("User:\nGreen.", "Green!  ", false),
        ("User:\nGreen notebook", "Green", false),
        ("User:\n?!", "?!", false),
        ("User:\n🌳", "🌳", false),
        ("User:\nAsk me.\n\nAssistant:\nGreen.", "Green.", false),
        (
            "User:\nAsk me.\n\nAssistant:\nGreen.",
            "me.\n\nAssistant:",
            false,
        ),
        ("Assistant:\nGreen.", "Green.", false),
        ("Bot:\nGreen.", "Green.", false),
        ("User:\nGreen.\n\nAssistant:\nGreen.", "Green.", false),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let mut store = store(&temp);
        record(&mut store, "short", "global", body, 100);
        let claim = "The speaker's answer has been recorded.";
        let mut l2 = action("create", "reply:short", "short", claim);
        l2["evidence"][0]["quote"] = quote.into();
        let prepared = store.learning_prepare(&request("short")).unwrap();
        let run = prepared["run_id"].as_str().unwrap();
        assert_eq!(
            store.learning_apply(run, input(vec![l2]), true).is_ok(),
            valid,
            "L2: {body:?} {quote:?}"
        );
        let mut l1 = proposal("reply:short", "short", claim);
        l1.evidence_quote = quote.into();
        assert_eq!(
            store.distill_memory(l1).is_ok(),
            valid,
            "L1: {body:?} {quote:?}"
        );
        assert_eq!(store.health().unwrap().active_memories, usize::from(valid));
    }
}

#[test]
fn newer_assistant_text_cannot_refresh_a_stale_human_correction() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = store(&temp);
    let old = "The Atlas deadline was Thursday.";
    let current = "The Atlas deadline is Friday.";
    let echo = "The Atlas deadline is Thursday.";
    record(&mut store, "old", "global", &format!("User:\n{old}"), 100);
    record(
        &mut store,
        "head",
        "global",
        &format!("User:\n{current}"),
        200,
    );
    let head = store
        .distill_memory(proposal("atlas:deadline", "head", current))
        .unwrap();
    record(
        &mut store,
        "later",
        "global",
        &format!("User:\nWhen is it?\n\nAssistant:\n{echo}"),
        300,
    );
    let mut bad_l1 = proposal("atlas:deadline", "later", echo);
    bad_l1.supersedes = Some(head.document_id);
    assert!(
        store
            .distill_memory(bad_l1)
            .unwrap_err()
            .to_string()
            .contains("original speaker")
    );

    let prepared = store.learning_prepare(&request("stale")).unwrap();
    let mut change = action("supersede", "atlas:deadline", "later", echo);
    change["target_document_id"] = head.document_id.into();
    let run = prepared["run_id"].as_str().unwrap();
    assert!(
        store
            .learning_apply(run, input(vec![change.clone()]), false)
            .unwrap_err()
            .to_string()
            .contains("original speaker")
    );
    change["evidence"]
        .as_array_mut()
        .unwrap()
        .push(json!({"session_id":"old","quote":old}));
    assert!(
        store
            .learning_apply(run, input(vec![change]), false)
            .unwrap_err()
            .to_string()
            .contains("newer than")
    );
    assert_eq!(store.health().unwrap().active_memories, 1);
}

#[test]
fn review_reports_readable_redacted_reason_and_source_identifiers() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = store(&temp);
    record(&mut store, "one", "global", "Atlas might use SQLite.", 100);
    let prepared = store.learning_prepare(&request("day")).unwrap();
    let mut value = action(
        "review",
        "atlas:database",
        "one",
        "Atlas has unresolved database evidence; api_key=private-review-canary",
    );
    value["evidence"][0]["quote"] = "Atlas might use SQLite.".into();
    store
        .learning_apply(
            prepared["run_id"].as_str().unwrap(),
            input(vec![value]),
            false,
        )
        .unwrap();
    let status = store.learning_status().unwrap();
    let reason = status["reviews"][0]["reason"].as_str().unwrap();
    assert!(reason.contains("unresolved database evidence"));
    assert!(!reason.contains("private-review-canary"));
    assert_eq!(status["reviews"][0]["evidence_session_ids"], json!(["one"]));
}

#[test]
fn manual_window_includes_both_bounds_and_excludes_other_scope_selection() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = store(&temp);
    record(
        &mut store,
        "old-other-scope",
        "private",
        "Private Atlas history.",
        99,
    );
    record(&mut store, "at-start", "global", "Atlas uses SQLite.", 100);
    record(
        &mut store,
        "at-end",
        "global",
        "Atlas supports local search.",
        200,
    );
    record(
        &mut store,
        "too-new",
        "global",
        "Atlas starts another task.",
        201,
    );
    let mut req = request("manual-window");
    req.after_ms = Some(100);
    req.before_ms = Some(200);
    req.preview = true;
    let prepared = store.learning_prepare(&req).unwrap();
    assert_eq!(prepared["scope"], "global");
    let sessions = prepared["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["selected"] == true)
        .map(|e| e["session_id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(sessions, vec!["at-start", "at-end"]);
    assert_eq!(store.learning_status().unwrap()["pending_evidence"], 4);
    req.after_ms = Some(201);
    assert!(store.learning_prepare(&req).is_err());
}

#[test]
fn recalled_review_lookup_uses_both_review_indexes() {
    let temp = tempfile::tempdir().unwrap();
    let store = store(&temp);
    let connection = rusqlite::Connection::open(store.path()).unwrap();
    let mut statement = connection
        .prepare(
            "EXPLAIN QUERY PLAN SELECT EXISTS(
            SELECT 1 FROM learning_reviews r WHERE r.document_id=d.id
                OR (r.document_id IS NULL AND r.scope=d.scope AND r.canonical_key=m.canonical_key)
         ) FROM documents d JOIN memory_items m ON m.document_id=d.id WHERE d.id=?1",
        )
        .unwrap();
    let plan = statement
        .query_map([1_i64], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
        .join("\n");
    assert!(plan.contains("learning_reviews_document"), "{plan}");
    assert!(plan.contains("learning_reviews_unassigned_key"), "{plan}");
}

fn selected_sessions(packet: &Value) -> Vec<&str> {
    packet["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["selected"] == true)
        .map(|item| item["session_id"].as_str().unwrap())
        .collect()
}

fn scheduled(key: &str, occurrence: &str) -> PrepareRequest {
    let mut req = request(key);
    req.occurrence = Some(occurrence.into());
    req
}

#[test]
fn rejected_evidence_gets_one_later_retry_and_fresh_work_keeps_advancing() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = store(&temp);
    record(
        &mut store,
        "vague",
        "global",
        "Jasper hummingbird enclosure temperature.",
        100,
    );
    let prepared = store
        .learning_prepare(&scheduled("day1:0", "day1"))
        .unwrap();
    let run = prepared["run_id"].as_str().unwrap();
    let rejection = input_with_deferred(vec![], vec!["vague".into()]);
    store.learning_apply(run, rejection.clone(), true).unwrap();
    store.learning_apply(run, rejection.clone(), false).unwrap();
    store.learning_apply(run, rejection, false).unwrap();

    record(
        &mut store,
        "fresh",
        "global",
        "Mailbox retention lasts thirty days.",
        200,
    );
    let fresh = store
        .learning_prepare(&scheduled("day1:1", "day1"))
        .unwrap();
    assert_eq!(selected_sessions(&fresh), ["fresh"]);
    let learned = store
        .learning_apply(
            fresh["run_id"].as_str().unwrap(),
            input(vec![action(
                "create",
                "mailbox:retention",
                "fresh",
                "Mailbox retention lasts thirty days.",
            )]),
            false,
        )
        .unwrap();
    assert_eq!(learned["processed_evidence"], 1);
    assert_eq!(
        store
            .learning_prepare(&scheduled("day1:2", "day1"))
            .unwrap()["status"],
        "empty"
    );

    // A failed model call and a dry run cannot spend the evidence retry.
    let second_day = scheduled("day2:0", "day2");
    let failed = store.learning_prepare(&second_day).unwrap();
    assert_eq!(selected_sessions(&failed), ["vague"]);
    store
        .learning_fail(failed["run_id"].as_str().unwrap())
        .unwrap();
    let retry = store.learning_prepare(&second_day).unwrap();
    assert_eq!(selected_sessions(&retry), ["vague"]);
    store
        .learning_apply(retry["run_id"].as_str().unwrap(), input(vec![]), true)
        .unwrap();
    let committed = store
        .learning_apply(retry["run_id"].as_str().unwrap(), input(vec![]), false)
        .unwrap();
    assert_eq!(committed["processed_evidence"], 0);
    assert_eq!(committed["deferred_session_ids"], json!(["vague"]));

    // Several batches in a different scope advance in the same occurrence,
    // even with the older source parked and small per-batch limits.
    for index in 0..12 {
        record(
            &mut store,
            &format!("fresh-{index}"),
            "maintenance",
            "Solar inverter passed the wiring inspection.",
            300 + index,
        );
    }
    for index in 0..4 {
        let mut req = scheduled(&format!("later:{index}"), "day2");
        req.limit = 3;
        let packet = store.learning_prepare(&req).unwrap();
        assert_eq!(packet["scope"], "maintenance");
        assert_eq!(selected_sessions(&packet).len(), 3);
        store
            .learning_apply(packet["run_id"].as_str().unwrap(), input(vec![]), false)
            .unwrap();
    }

    let path = store.path().to_owned();
    drop(store);
    let mut store = Store::open(&path, 64).unwrap();
    for day in ["day3", "day4"] {
        assert_eq!(
            store.learning_prepare(&scheduled(day, day)).unwrap()["status"],
            "empty"
        );
    }
    assert_eq!(store.learning_status().unwrap()["pending_evidence"], 1);
    assert_eq!(store.health().unwrap().active_memories, 1);
}

#[test]
fn only_related_new_evidence_in_scope_reopens_a_parked_record_and_it_is_shown() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = store(&temp);
    let source = "The Jasper hummingbird enclosure requires amber insulation.";
    record(&mut store, "parked", "aviary", source, 100);
    for day in ["day1", "day2"] {
        let packet = store.learning_prepare(&scheduled(day, day)).unwrap();
        assert_eq!(selected_sessions(&packet), ["parked"]);
        store
            .learning_apply(
                packet["run_id"].as_str().unwrap(),
                input_with_deferred(vec![], vec!["parked".into()]),
                false,
            )
            .unwrap();
    }
    record(&mut store, "foreign", "other", source, 110);
    record(
        &mut store,
        "unrelated",
        "aviary",
        "Spring potatoes thrive in sandy soil.",
        120,
    );
    for day in ["day3", "day4"] {
        let packet = store.learning_prepare(&scheduled(day, day)).unwrap();
        assert!(!selected_sessions(&packet).contains(&"parked"));
        store
            .learning_apply(packet["run_id"].as_str().unwrap(), input(vec![]), false)
            .unwrap();
    }
    assert_eq!(
        store.learning_prepare(&request("idle")).unwrap()["status"],
        "empty"
    );

    // The trigger is newly ingested, even when its source timestamp predates
    // the rejection. Both sources must be available when the model reassesses.
    record(
        &mut store,
        "corroboration",
        "aviary",
        "The Jasper hummingbird enclosure requires amber insulation during winter.",
        90,
    );
    let mut req = scheduled("related:0", "related");
    req.limit = 1;
    let fresh = store.learning_prepare(&req).unwrap();
    assert_eq!(selected_sessions(&fresh), ["corroboration"]);
    store
        .learning_apply(fresh["run_id"].as_str().unwrap(), input(vec![]), false)
        .unwrap();

    req.run_key = "related:1".into();
    let retry = store.learning_prepare(&req).unwrap();
    assert_eq!(selected_sessions(&retry), ["parked"]);
    assert!(
        retry["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["session_id"] == "corroboration" && item["selected"] == false)
    );
    // A new observation arrives while the model is reviewing the first trigger.
    // The empty apply may consume only the supporting evidence it actually saw.
    let confirmation = "The Jasper hummingbird enclosure requires amber insulation.";
    record(&mut store, "confirmation", "aviary", confirmation, 210);
    store
        .learning_apply(retry["run_id"].as_str().unwrap(), input(vec![]), false)
        .unwrap();

    req.run_key = "related:2".into();
    let fresh = store.learning_prepare(&req).unwrap();
    assert_eq!(selected_sessions(&fresh), ["confirmation"]);
    store
        .learning_apply(fresh["run_id"].as_str().unwrap(), input(vec![]), false)
        .unwrap();
    req.run_key = "related:3".into();
    let retry = store.learning_prepare(&req).unwrap();
    assert_eq!(selected_sessions(&retry), ["parked"]);
    assert!(
        retry["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["session_id"] == "confirmation" && item["selected"] == false)
    );
    let mut fact = action("create", "aviary:insulation", "parked", source);
    fact["evidence"]
        .as_array_mut()
        .unwrap()
        .push(json!({"session_id":"confirmation","quote":confirmation}));
    let committed = store
        .learning_apply(retry["run_id"].as_str().unwrap(), input(vec![fact]), false)
        .unwrap();
    assert_eq!(committed["processed_evidence"], 1);
    assert_eq!(store.learning_status().unwrap()["pending_evidence"], 0);
    assert_eq!(store.health().unwrap().citations, 2);
    assert_eq!(
        store.learning_prepare(&request("next")).unwrap()["status"],
        "empty"
    );
}

#[test]
fn rule_change_and_targeted_operator_review_each_allow_one_reconsideration() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = store(&temp);
    record(&mut store, "first", "global", "Atlas uses SQLite.", 100);
    record(
        &mut store,
        "second",
        "global",
        "Ibis monitors wind speed.",
        101,
    );
    let both = vec!["first".to_owned(), "second".to_owned()];
    for day in ["one", "two"] {
        let packet = store.learning_prepare(&scheduled(day, day)).unwrap();
        store
            .learning_apply(
                packet["run_id"].as_str().unwrap(),
                input_with_deferred(vec![], both.clone()),
                false,
            )
            .unwrap();
    }
    assert_eq!(
        store.learning_prepare(&request("unchanged")).unwrap()["status"],
        "empty"
    );
    let mut rule_change = request("rule-change");
    rule_change.rule_version = "rules-b".into();
    let packet = store.learning_prepare(&rule_change).unwrap();
    assert_eq!(selected_sessions(&packet), ["first", "second"]);
    let deferred = store
        .learning_apply(packet["run_id"].as_str().unwrap(), input(vec![]), false)
        .unwrap();
    assert_eq!(deferred["deferred_evidence"], 2);
    rule_change.run_key = "unchanged-again".into();
    assert_eq!(
        store.learning_prepare(&rule_change).unwrap()["status"],
        "empty"
    );

    // The operator can discover an older deferral without retrieving its text.
    let listing = moon(temp.path())
        .args(["learning", "deferred", "--limit", "1"])
        .assert()
        .success();
    let page: Value = serde_json::from_slice(&listing.get_output().stdout).unwrap();
    assert_eq!(page["deferred"][0]["session_id"], "first");
    assert_eq!(page["deferred"][0]["state"], "parked");
    assert!(!page.to_string().contains("SQLite"));
    let next = store
        .learning_deferred(page["next_after_id"].as_i64().unwrap(), 1)
        .unwrap();
    assert_eq!(next["deferred"][0]["session_id"], "second");
    assert!(next["next_after_id"].is_null());

    // Exercise the operator's actual CLI entry point and fresh-process replay.
    let output = moon(temp.path())
        .args([
            "learning",
            "prepare",
            "--run-key",
            "review-first",
            "--rule-version",
            "rules-b",
            "--review-session",
            "first",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let review: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(selected_sessions(&review), ["first"]);
    let accepted = store
        .learning_apply(
            review["run_id"].as_str().unwrap(),
            input(vec![action(
                "create",
                "atlas:sqlite",
                "first",
                "Atlas uses SQLite.",
            )]),
            false,
        )
        .unwrap();
    assert_eq!(accepted["processed_evidence"], 1);
    assert_eq!(store.learning_status().unwrap()["pending_evidence"], 1);
    assert_eq!(
        store.learning_prepare(&rule_change).unwrap()["status"],
        "empty"
    );
}

#[test]
fn migration_restores_only_attributable_unprocessed_legacy_deferrals() {
    let temp = tempfile::tempdir().unwrap();
    // Use the actual v13 schema so this regression still exercises the upgrade
    // when later migrations change how a newly initialised store is laid out.
    drop(store(&temp)); // Register Moon's bundled SQLite vector extension.
    let path = temp.path().join("v13.sqlite");
    let db = rusqlite::Connection::open(&path).unwrap();
    for (index, sql) in [
        include_str!("../migrations/0001_init.sql"),
        include_str!("../migrations/0002_canonical_body.sql"),
        include_str!("../migrations/0003_fts_source_kind.sql"),
        include_str!("../migrations/0004_evidence_memory.sql"),
        include_str!("../migrations/0005_runtime_safety.sql"),
        include_str!("../migrations/0006_auto_embedding.sql"),
        include_str!("../migrations/0007_context_metrics.sql"),
        include_str!("../migrations/0008_learning_lifecycle.sql"),
        include_str!("../migrations/0009_source_storage.sql"),
        include_str!("../migrations/0010_source_learning.sql"),
        include_str!("../migrations/0011_topic_synthesis.sql"),
        include_str!("../migrations/0012_retrieval_validity.sql"),
        include_str!("../migrations/0013_job_diagnostics.sql"),
    ]
    .into_iter()
    .enumerate()
    {
        db.execute_batch(sql).unwrap();
        db.execute(
            "INSERT INTO schema_migrations VALUES(?1, 1)",
            [(index + 1) as i64],
        )
        .unwrap();
    }
    db.execute_batch("PRAGMA user_version=13;
        INSERT INTO metadata VALUES('embedding_dimensions','64');
        CREATE VIRTUAL TABLE chunk_vectors USING vec0(embedding float[64], source_kind text, scope text);").unwrap();
    for session in ["twice", "once", "processed", "unseen"] {
        db.execute(
            "INSERT INTO documents(source_uri,source_kind,scope,content_hash,
                modified_at_ms,indexed_at_ms,body) VALUES(?1,'evidence','archive','hash',1,1,?2)",
            rusqlite::params![
                format!("evidence://{session}"),
                "Archive notes for the standing order."
            ],
        )
        .unwrap();
        db.execute(
            "INSERT INTO evidence_sessions(session_id,document_id,completed_at_ms,recorded_at_ms)
            VALUES(?1,?2,1,1)",
            rusqlite::params![session, db.last_insert_rowid()],
        )
        .unwrap();
    }
    for (day, deferred) in [
        (1, json!(["twice", "processed"])),
        (2, json!(["twice", "once"])),
    ] {
        let key = format!("l2:2026-11-{day:02}:0");
        db.execute("INSERT INTO learning_runs(run_id,run_key,scope,status,created_at_ms,
            completed_at_ms,snapshot_hash,snapshot_json,outcome_json)
            VALUES(?1,?1,'archive','committed',?2,?2,'hash','{}',?3)",
            rusqlite::params![key, day,
                json!({"deferred_session_ids":deferred,"deferred_evidence":deferred.as_array().unwrap().len()}).to_string()],
        ).unwrap();
    }
    db.execute(
        "INSERT INTO learning_processed_evidence(evidence_session_id, run_id, processed_at_ms)
        SELECT id, 'l2:2026-11-02:0', 2 FROM evidence_sessions WHERE session_id='processed'",
        [],
    )
    .unwrap();
    drop(db);

    let mut store = Store::open(&path, 64).unwrap();
    assert_eq!(store.learning_status().unwrap()["pending_evidence"], 3);
    let mut selected = Vec::new();
    for batch in 0..3 {
        let packet = store
            .learning_prepare(&scheduled(&format!("day3:{batch}"), "2026-11-03"))
            .unwrap();
        if packet["status"] == "empty" {
            break;
        }
        selected.extend(selected_sessions(&packet).into_iter().map(str::to_owned));
        store
            .learning_apply(packet["run_id"].as_str().unwrap(), input(vec![]), false)
            .unwrap();
    }
    selected.sort();
    assert_eq!(selected, ["once", "unseen"]);
    assert_eq!(store.learning_status().unwrap()["pending_evidence"], 2);
    assert_eq!(
        store.learning_prepare(&request("day4")).unwrap()["status"],
        "empty"
    );
    assert!(store.health().unwrap().ok);
}
