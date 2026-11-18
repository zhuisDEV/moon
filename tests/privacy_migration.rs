use moon::learning::PrepareRequest;
use moon::{DistillInput, EvidenceInput, IngestDocument, MemoryInput, Store};
use rusqlite::Connection;
use serde_json::json;

fn readers(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| format!("discord:{name}")).collect()
}

#[test]
fn verified_readers_use_subset_semantics_and_unknown_gets_only_public() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("test.sqlite"), 64).unwrap();
    let public = store.public_memory_scope().unwrap();
    let a = store.register_private_audience(&readers(&["a"])).unwrap();
    let ab = store
        .register_private_audience(&readers(&["a", "b"]))
        .unwrap();
    assert_eq!(
        ab,
        store
            .register_private_audience(&readers(&["b", "a", "a"]))
            .unwrap()
    );
    assert!(store.register_private_audience(&[]).is_err());
    assert!(store.register_private_audience(&["".into()]).is_err());
    assert_eq!(
        store.readable_memory_scopes(&[]).unwrap(),
        vec![public.clone()]
    );
    assert_eq!(
        store.readable_memory_scopes(&readers(&["a", "b"])).unwrap(),
        vec![public.clone(), ab.clone()]
    );
    let only_a = store.readable_memory_scopes(&readers(&["a"])).unwrap();
    assert!(only_a.contains(&a) && only_a.contains(&ab));
    assert_eq!(
        store.readable_memory_scopes(&readers(&["b"])).unwrap(),
        vec![public.clone(), ab]
    );
    assert_eq!(
        store
            .readable_memory_scopes(&readers(&["a", "b", "c"]))
            .unwrap(),
        vec![public]
    );
}

#[test]
fn copy_migration_quarantines_history_and_recovers_only_a_reviewed_user_quote() {
    const USER: &str = "The rehearsal starts at noon.";
    const ASSISTANT: &str = "Private context from an unrelated legacy chat.";
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("original.sqlite");
    let copy = dir.path().join("rehearsal.sqlite");
    let mut store = Store::open(&original, 64).unwrap();
    store
        .record_evidence(EvidenceInput {
            session_id: "original-session".into(),
            scope: "global".into(),
            title: None,
            content: format!("User:\n{USER}\n\nAssistant:\n{ASSISTANT}"),
            completed_at_ms: 12345,
            metadata_json: "{}".into(),
        })
        .unwrap();
    let claim = |quote: &str, key: &str| DistillInput {
        canonical_key: key.into(),
        scope: "global".into(),
        memory_kind: "fact".into(),
        content: quote.into(),
        title: None,
        importance: 0.9,
        confidence: 1.0,
        pinned: true,
        evidence_session_id: "original-session".into(),
        evidence_quote: quote.into(),
        supersedes: None,
        valid_until_ms: Some(4_102_444_800_000),
    };
    let mine = store
        .distill_memory(claim(USER, "legacy:mine"))
        .unwrap()
        .document_id;
    let other = store
        .distill_memory(claim(ASSISTANT, "legacy:other"))
        .unwrap()
        .document_id;
    // Schema 14 allowed arbitrary text scope names, including the new defaults.
    let alice = readers(&["a"]);
    let old_private = store.register_private_audience(&alice).unwrap();
    for scope in ["public", old_private.as_str()] {
        store
            .remember(MemoryInput {
                scope: scope.into(),
                memory_kind: "fact".into(),
                content: "Unverified old claim".into(),
                title: None,
                importance: 0.5,
                confidence: 1.0,
                pinned: true,
            })
            .unwrap();
    }
    store.backup_to(&copy).unwrap();
    drop(store);
    let db = Connection::open(&copy).unwrap();
    db.execute_batch(concat!(
        include_str!("fixtures/pre_speaker_schema.sql"),
        include_str!("fixtures/pre_audience_schema.sql")
    ))
    .unwrap();
    // The old adapter saved these host markers before verified admissions existed.
    db.execute(
        "UPDATE evidence_sessions SET metadata_json=?1 WHERE session_id='original-session'",
        [json!({"source":"openclaw","session_key":"fixture","admission_entry_id":"start","terminal_entry_id":"end"}).to_string()],
    ).unwrap();
    // A prepared old writer must not be able to publish after cutover.
    db.execute("INSERT INTO learning_runs(run_id,run_key,scope,status,created_at_ms,lease_until_ms,snapshot_hash,snapshot_json)
                VALUES('old-run','old-schedule','global','prepared',100,4102444800000,'opaque','{}')",[]).unwrap();
    drop(db);
    let mut store = Store::open(&copy, 64).unwrap();
    assert_ne!(store.public_memory_scope().unwrap(), "public");
    assert_ne!(
        store.register_private_audience(&alice).unwrap(),
        old_private
    );
    assert_eq!(
        store.legacy_memory_inventory().unwrap(),
        json!({"legacy_documents":3,"legacy_memories":2,"legacy_evidence":1,"privately_recovered":0})
    );
    assert_eq!(store.learning_status().unwrap()["pending_evidence"], 0);
    let prepare = PrepareRequest {
        run_key: "new".into(),
        rule_version: "".into(),
        review_session: None,
        occurrence: None,
        limit: 16,
        max_chars: 8192,
        lease_ms: 60_000,
        max_attempts: 3,
        before_ms: None,
        after_ms: None,
        preview: false,
    };
    assert_eq!(store.learning_prepare(&prepare).unwrap()["status"], "empty");
    let db = Connection::open(&copy).unwrap();
    let count = || {
        db.query_row(
            "SELECT count(*) FROM audience_recallable_memories",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
    };
    assert_eq!(count(), 0);
    assert!(
        store
            .recover_legacy_memory(other, &alice, ASSISTANT)
            .is_err()
    );
    assert!(store.recover_legacy_memory(mine, &[], USER).is_err());
    let candidates = store.legacy_memory_candidates(0, 100).unwrap();
    assert_eq!(candidates.len(), 2);
    assert!(
        candidates
            .iter()
            .all(|c| c.get("quote").is_none() && c["recovered"] == false)
    );
    assert_eq!(store.legacy_memory_review(mine).unwrap()["quote"], USER);
    assert!(store.legacy_memory_review(other).is_err());
    let recovered = store.recover_legacy_memory(mine, &alice, USER).unwrap();
    assert_eq!(count(), 1);
    assert!(store.assign_legacy_collection("discord:b").is_err());
    assert_eq!(
        store.recover_legacy_memory(mine, &alice, USER).unwrap(),
        recovered
    );
    assert!(
        store
            .recover_legacy_memory(mine, &readers(&["b"]), USER)
            .is_err()
    );
    assert_eq!(
        db.query_row(
            "SELECT valid_until_ms FROM memory_items WHERE document_id=?1",
            [recovered],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        4_102_444_800_000
    );
    let copied_evidence: String = db
        .query_row(
            "SELECT d.body FROM memory_citations c JOIN documents d ON d.id=c.evidence_document_id
                           WHERE c.memory_document_id=?1",
            [recovered],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(copied_evidence, USER);
    let uri: String = db
        .query_row(
            "SELECT source_uri FROM documents WHERE id=?1",
            [recovered],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        store
            .ingest(IngestDocument {
                source_uri: uri,
                source_kind: "memory".into(),
                scope: store.public_memory_scope().unwrap(),
                title: None,
                content: "Replacement statement".into(),
                modified_at_ms: 54321,
                metadata_json: "{}".into()
            })
            .is_err()
    );
    assert!(store.health().unwrap().ok);
    drop(store);
    let reopened = Store::open_existing(copy, 64).unwrap();
    assert_eq!(
        reopened.legacy_memory_inventory().unwrap()["privately_recovered"],
        1
    );
    assert_eq!(
        Store::open_existing(original, 64)
            .unwrap()
            .legacy_memory_inventory()
            .unwrap()["privately_recovered"],
        0
    );
}

#[test]
fn owner_attestation_grants_only_saved_legacy_memories_and_never_invents_evidence() {
    use moon::{ContextRequest, SearchMode};

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("attest.sqlite");
    let mut store = Store::open(&path, 64).unwrap();
    let raw_quote = "The original source also included an unrelated sentence.";
    store
        .record_evidence(EvidenceInput {
            session_id: "original".into(),
            scope: "global".into(),
            title: None,
            content: raw_quote.into(),
            completed_at_ms: 10,
            metadata_json: "{}".into(),
        })
        .unwrap();
    store
        .distill_memory(DistillInput {
            canonical_key: "itinerary:one".into(),
            memory_kind: "fact".into(),
            scope: "global".into(),
            title: None,
            content: "The itinerary contains a harbour visit.".into(),
            importance: 0.8,
            confidence: 1.0,
            pinned: false,
            evidence_session_id: "original".into(),
            evidence_quote: raw_quote.into(),
            supersedes: None,
            valid_until_ms: None,
        })
        .unwrap();
    let remember = |store: &mut Store, text: &str| {
        store
            .remember(MemoryInput {
                scope: "global".into(),
                memory_kind: "fact".into(),
                content: text.into(),
                title: None,
                importance: 0.8,
                confidence: 1.0,
                pinned: false,
            })
            .unwrap()
            .document_id
    };
    remember(&mut store, "The uncited itinerary lists a harbour stop.");
    let expired = remember(&mut store, "The obsolete itinerary listed a harbour tram.");
    let db = Connection::open(&path).unwrap();
    db.execute(
        "UPDATE memory_items SET valid_until_ms=1 WHERE document_id=?1",
        [expired],
    )
    .unwrap();
    // Capture a representative pre-cutover cohort; new global writes never enter it.
    db.execute(
        "INSERT INTO privacy_legacy_documents SELECT id FROM documents WHERE scope='global'",
        [],
    )
    .unwrap();
    remember(
        &mut store,
        "The future global itinerary lists a harbour bridge.",
    );
    let owner = "discord:10000000000000001";
    let other = "discord:10000000000000002";
    let expected = json!({"owner":owner,"assigned":3,"recallable":2});
    assert_eq!(store.assign_legacy_collection(owner).unwrap(), expected);
    drop(store);
    let mut store = Store::open(&path, 64).unwrap();
    assert_eq!(store.assign_legacy_collection(owner).unwrap(), expected);
    assert!(store.assign_legacy_collection(other).is_err());
    let request = ContextRequest {
        query: "itinerary harbour".into(),
        mode: SearchMode::Lexical,
        scope: None,
        limit: 10,
        max_chars: 8000,
        evidence_per_memory: 5,
    };
    let packet = store
        .assemble_audience_context(&request, None, &[owner.into()])
        .unwrap();
    assert_eq!(packet.memories.len(), 2);
    for memory in &packet.memories {
        assert!(memory.owner_attested_legacy);
        assert!(memory.citations.is_empty());
        assert_ne!(memory.scope, "global");
    }
    let rendered = packet.render_markdown();
    assert!(rendered.contains("Original speaker and authorship are unverified"));
    assert!(!rendered.contains(raw_quote));
    assert!(!rendered.contains("future global"));
    assert!(!rendered.contains("obsolete itinerary"));
    for audience in [vec![], vec![other.into()], vec![owner.into(), other.into()]] {
        assert!(
            store
                .assemble_audience_context(&request, None, &audience)
                .unwrap()
                .is_empty()
        );
    }
    let totals: (i64, i64) = db
        .query_row(
            "SELECT (SELECT count(*) FROM evidence_sessions),
                                    (SELECT count(*) FROM evidence_admissions)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(totals, (1, 0));
    assert!(store.evidence_by_sender(owner, 0, 10).unwrap().is_empty());
    assert!(store.health().unwrap().ok);
}
