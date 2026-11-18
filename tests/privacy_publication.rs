use moon::{ContextRequest, SearchMode, Store};
use serde_json::Value;
use tempfile::tempdir;

const ACTOR: &str = "discord:10000000000000001";
const OTHER: &str = "discord:10000000000000002";

fn recall(store: &Store, query: &str) -> String {
    store
        .assemble_audience_context(
            &ContextRequest {
                query: query.into(),
                mode: SearchMode::Lexical,
                scope: None,
                limit: 8,
                max_chars: 3500,
                evidence_per_memory: 2,
            },
            None,
            &[],
        )
        .unwrap()
        .render_markdown()
}

#[test]
fn public_wording_and_authorship_survive_retry_withdrawal_and_restart() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("moon.sqlite");
    let mut store = Store::open(&path, 8).unwrap();
    assert!(
        store
            .publish_memory(ACTOR, "fact", "password=super-secret-12345", None)
            .is_err()
    );
    let wording = "Use a coral compass for telescope alignment.";
    let event = Some("message-one");
    let first = store
        .publish_memory(ACTOR, "skill", wording, event)
        .unwrap();
    assert_eq!(
        first,
        store
            .publish_memory(ACTOR, "skill", wording, event)
            .unwrap()
    );
    assert!(
        store
            .publish_memory(ACTOR, "fact", "Other wording", event)
            .is_err()
    );
    assert!(recall(&store, "coral telescope").contains(wording));
    assert_eq!(store.public_memories(0, 10).unwrap().len(), 1);
    let id = first["document_id"].as_i64().unwrap();
    assert!(store.withdraw_publication(OTHER, id).is_err());
    assert_eq!(
        store.withdraw_publication(ACTOR, id).unwrap()["active"],
        false
    );
    drop(store);
    let mut store = Store::open(&path, 8).unwrap();
    assert!(store.public_memories(0, 10).unwrap().is_empty());
    assert!(!recall(&store, "coral telescope").contains(wording));
    assert_eq!(
        store
            .publish_memory(ACTOR, "skill", wording, event)
            .unwrap()["active"],
        Value::Bool(false)
    );
}

#[test]
fn accepted_turn_audience_cannot_be_reassigned_on_retry() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("moon.sqlite");
    let mut store = Store::open(&path, 8).unwrap();
    let key = "a".repeat(64);
    let one = vec![ACTOR.into()];
    assert!(store.turn_admission(&key).unwrap().audience.is_none());
    assert!(store.bind_turn_owners(&key, &[], ACTOR, &one).is_err());
    store.bind_turn_owners(&key, &one, ACTOR, &one).unwrap();
    drop(store);
    let mut store = Store::open(&path, 8).unwrap();
    assert!(
        store
            .bind_turn_owners(&key, &[ACTOR.into(), OTHER.into()], ACTOR, &one)
            .is_err()
    );
    let binding = store.turn_admission(&key).unwrap();
    assert_eq!(binding.sender.as_deref(), Some(ACTOR));
    assert_eq!(binding.audience.as_ref(), Some(&one));
    assert_eq!(binding.owners.as_ref(), Some(&one));
}

#[test]
fn original_owners_are_independent_of_silent_potential_readers() {
    let dir = tempdir().unwrap();
    let mut store = Store::open(dir.path().join("moon.sqlite"), 8).unwrap();
    let key = "a".repeat(64);
    let owners = vec![ACTOR.to_owned(), OTHER.to_owned()];
    let mut readers = owners.clone();
    readers.push("discord:10000000000000003".into());
    assert!(
        store
            .bind_turn_owners(&key, &owners, ACTOR, &readers)
            .is_err()
    );
    store
        .bind_turn_owners(&key, &readers, ACTOR, &owners)
        .unwrap();
    assert!(
        store
            .bind_turn_owners(&key, &readers, ACTOR, &readers)
            .is_err()
    );
    let turn = store.turn_admission(&key).unwrap();
    assert_eq!(turn.audience.as_ref(), Some(&readers));
    assert_eq!(turn.owners.as_ref(), Some(&owners));
    let scope = store.register_private_audience(&owners).unwrap();
    assert!(
        store
            .readable_memory_scopes(&owners)
            .unwrap()
            .contains(&scope)
    );
    assert!(
        !store
            .readable_memory_scopes(&readers)
            .unwrap()
            .contains(&scope)
    );
}

#[test]
fn private_record_skips_the_entire_credential_bearing_turn() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("rehearsal.sqlite");
    let mut command = assert_cmd::Command::new(assert_cmd::cargo::cargo_bin!("moon"));
    let result = command
        .arg("--home")
        .arg(dir.path())
        .arg("--database")
        .arg(&db)
        .args([
            "--json",
            "record",
            "--session-id",
            "credential-turn",
            "--audience-json",
        ])
        .arg(serde_json::to_string(&[ACTOR]).unwrap())
        .write_stdin("User:\nMy password is synthetic words.\nA separate detail also stays out.")
        .assert()
        .success();
    let receipt: Value = serde_json::from_slice(&result.get_output().stdout).unwrap();
    assert_eq!(receipt["changed"], false);
    assert!(receipt["redactions"].as_u64().unwrap() > 0);
    let conn = rusqlite::Connection::open(db).unwrap();
    for table in ["evidence_sessions", "documents", "chunks", "memory_items"] {
        let rows: i64 = conn
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0, "unexpected credential-turn data in {table}");
    }
}

#[test]
fn authenticated_sender_survives_shared_room_replay_without_changing_its_audience() {
    use moon::EvidenceInput;
    let dir = tempdir().unwrap();
    let db = dir.path().join("room.sqlite");
    let mut store = Store::open(&db, 8).unwrap();
    let readers = vec![ACTOR.to_owned(), OTHER.to_owned()];
    let room = store.register_private_audience(&readers).unwrap();
    let single = store.register_private_audience(&[ACTOR.into()]).unwrap();
    let first = "a".repeat(64);
    let second = "b".repeat(64);
    let third = "c".repeat(64);
    assert!(
        store
            .bind_turn_owners(&first, &readers, "discord:absent", &readers)
            .is_err()
    );
    for (key, sender) in [(&first, ACTOR), (&second, OTHER), (&third, ACTOR)] {
        store
            .bind_turn_owners(key, &readers, sender, &readers)
            .unwrap();
    }
    assert!(
        store
            .bind_turn_owners(&first, &readers, OTHER, &readers)
            .is_err()
    );
    let turn = |id: &str, time: i64| EvidenceInput {
        session_id: id.into(),
        // The stored wording and caller's metadata must never supply verified identity.
        content: "User:\nMy name in this message is someone else.".into(),
        title: None,
        scope: room.clone(),
        completed_at_ms: time,
        metadata_json: "{}".into(),
    };
    let mut old_adapter = turn("unbound-host", 1);
    old_adapter.metadata_json = serde_json::json!({"source":"openclaw"}).to_string();
    assert!(store.record_evidence(old_adapter).is_err());
    let mut pre_audience_adapter = turn("old-global-host", 2);
    pre_audience_adapter.scope = "global".into();
    pre_audience_adapter.metadata_json = serde_json::json!({"source":"openclaw"}).to_string();
    assert!(store.record_evidence(pre_audience_adapter).is_err());
    let mut forged = turn("forged", 1);
    forged.metadata_json = serde_json::json!({"verified_sender":OTHER}).to_string();
    assert!(store.record_turn_evidence(forged, Some(&first)).is_err());
    let mut wrong_room = turn("wrong-room", 1);
    wrong_room.scope = single;
    assert!(
        store
            .record_turn_evidence(wrong_room, Some(&first))
            .is_err()
    );
    store
        .record_turn_evidence(turn("second-by-time", 20), Some(&first))
        .unwrap();
    store
        .record_turn_evidence(turn("other", 10), Some(&second))
        .unwrap();
    store
        .record_turn_evidence(turn("first-by-time", 1), Some(&third))
        .unwrap();
    store.record_evidence(turn("operator", 12)).unwrap();
    assert!(
        store
            .record_turn_evidence(turn("duplicate-admission", 21), Some(&first))
            .is_err()
    );
    drop(store);
    let mut store = Store::open(&db, 8).unwrap();
    assert!(
        !store
            .record_turn_evidence(turn("second-by-time", 20), Some(&first))
            .unwrap()
            .changed
    );
    assert!(
        store
            .record_turn_evidence(turn("second-by-time", 20), Some(&second))
            .is_err()
    );
    assert!(
        store
            .record_turn_evidence(turn("operator", 12), Some(&second))
            .is_err()
    );
    let page = store.evidence_by_sender(ACTOR, 0, 1).unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0]["completed_at_ms"], 1);
    assert_eq!(page[0]["reader_count"], 2);
    assert_eq!(page[0]["citing_memories"], 0);
    assert_eq!(page[0]["owners"], serde_json::json!(readers));
    let cursor = page[0]["evidence_id"].as_i64().unwrap();
    let rest = store.evidence_by_sender(ACTOR, cursor, 100).unwrap();
    assert_eq!(rest.len(), 1);
    assert_eq!(rest[0]["completed_at_ms"], 20);
    assert!(store.evidence_by_sender(OTHER, cursor, 100).is_err());
    let theirs = store.evidence_by_sender(OTHER, 0, 100).unwrap();
    assert_eq!(theirs.len(), 1);
    assert_eq!(theirs[0]["completed_at_ms"], 10);
}

#[test]
fn schema_fifteen_admissions_remain_unattributed_after_migration() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("v15.sqlite");
    let mut store = Store::open(&db, 8).unwrap();
    let single = store.register_private_audience(&[ACTOR.into()]).unwrap();
    let old_evidence = |metadata: &str| moon::EvidenceInput {
        session_id: "pre-migration".into(),
        scope: single.clone(),
        title: None,
        content: "User:\nA historical turn.".into(),
        completed_at_ms: 100,
        metadata_json: metadata.into(),
    };
    store.record_evidence(old_evidence("{}")).unwrap();
    drop(store);
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch(include_str!("fixtures/pre_speaker_schema.sql"))
        .unwrap();
    conn.execute_batch(
        "
DROP VIEW chat_recallable_memories;
DROP VIEW owned_legacy_memories;
DROP TRIGGER privacy_recovery_owner_scope;
DROP TABLE legacy_collection_owner;
DROP TABLE private_turn_history;
DELETE FROM schema_migrations WHERE version IN (17,18);
        DROP TABLE admission_room_activity;
DROP TRIGGER privacy_admissions_require_owners;
        DROP TABLE evidence_admissions;
        ALTER TABLE privacy_admissions DROP COLUMN room_access_at_ms;
        ALTER TABLE privacy_admissions DROP COLUMN owners_json;
        DROP TRIGGER privacy_admissions_require_sender;
        DROP TRIGGER privacy_admissions_immutable;
        DROP TRIGGER privacy_admissions_retained;
        DROP INDEX privacy_admissions_sender;
        ALTER TABLE privacy_admissions DROP COLUMN sender;
        DELETE FROM schema_migrations WHERE version=16;
        PRAGMA user_version=15;",
    )
    .unwrap();
    // Model the immutable host metadata actually stored by a pre-16 private recorder.
    let old_metadata = r#"{"source":"openclaw"}"#;
    conn.execute(
        "UPDATE evidence_sessions SET metadata_json=?1 WHERE session_id='pre-migration'",
        [old_metadata],
    )
    .unwrap();
    let key = "e".repeat(64);
    conn.execute(
        "INSERT INTO privacy_admissions(admission_key,audience_json) VALUES(?1,?2)",
        rusqlite::params![key, serde_json::to_string(&[ACTOR]).unwrap()],
    )
    .unwrap();
    drop(conn);
    let mut store = Store::open(&db, 8).unwrap();
    assert!(
        !store
            .record_evidence(old_evidence(old_metadata))
            .unwrap()
            .changed
    );
    let reader = vec![ACTOR.to_owned()];
    let legacy = store.turn_admission(&key).unwrap();
    assert_eq!(legacy.audience.as_ref(), Some(&reader));
    assert!(legacy.sender.is_none());
    assert!(legacy.owners.is_none());
    assert!(
        store
            .bind_turn_owners(&key, &reader, ACTOR, &reader)
            .is_err()
    );
    assert!(store.evidence_by_sender(ACTOR, 0, 20).unwrap().is_empty());
}
