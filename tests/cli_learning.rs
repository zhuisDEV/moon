use assert_cmd::Command;
use moon::learning::{ApplyInput, PrepareRequest};
use moon::{ContextRequest, DistillAction, DistillInput, EvidenceInput, SearchMode, Store};
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
}

#[test]
fn l2_rejects_negation_hypothetical_and_unsupported_numbers() {
    for (quote, content) in [
        ("No Atlas API key is stored.", "Atlas API key is stored."),
        (
            "If Atlas uses SQLite, this is hypothetical.",
            "Atlas uses SQLite.",
        ),
        ("Atlas previously used Live Room.", "Atlas uses Live Room."),
        ("Atlas supports 20 tools.", "Atlas supports 200 tools."),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let mut store = store(&temp);
        record(&mut store, "one", "global", quote, 100);
        let prepared = store.learning_prepare(&request("day")).unwrap();
        let mut value = action("create", "atlas:claim", "one", content);
        value["evidence"][0]["quote"] = quote.into();
        assert!(
            store
                .learning_apply(
                    prepared["run_id"].as_str().unwrap(),
                    input(vec![value]),
                    false
                )
                .is_err(),
            "{quote} -> {content}"
        );
        assert_eq!(store.health().unwrap().active_memories, 0);
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
    connection.execute("INSERT INTO memory_heads(canonical_key,document_id,updated_at_ms) VALUES('atlas:legacy',?1,100)",[duplicate]).unwrap();
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
fn l1_rejects_scope_and_kind_collisions() {
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
    assert!(store.distill_memory(cross).is_err());
    let mut kind = proposal("atlas:database", "one", "Atlas uses SQLite.");
    kind.memory_kind = "preference".into();
    assert!(store.distill_memory(kind).is_err());
    assert_eq!(store.health().unwrap().active_memories, 1);
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
fn unrelated_new_quote_cannot_make_an_old_claim_fresh() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = store(&temp);
    record(
        &mut store,
        "old",
        "global",
        "Atlas stores its API key in shared configuration.",
        100,
    );
    record(
        &mut store,
        "new",
        "global",
        "Atlas completed unrelated mailbox maintenance.",
        200,
    );
    let prepared = store.learning_prepare(&request("day")).unwrap();
    let mut value = action(
        "create",
        "atlas:key",
        "old",
        "Atlas stores its API key in shared configuration.",
    );
    value["evidence"]
        .as_array_mut()
        .unwrap()
        .push(json!({"session_id":"new","quote":"Atlas completed unrelated mailbox maintenance."}));
    assert!(
        store
            .learning_apply(
                prepared["run_id"].as_str().unwrap(),
                input(vec![value]),
                false
            )
            .is_err()
    );
    assert_eq!(store.health().unwrap().active_memories, 0);
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
