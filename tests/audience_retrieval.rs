use std::collections::BTreeSet;

use moon::{
    ContextRequest, DistillInput, EvidenceInput, HashEmbedding, IngestDocument, MemoryInput,
    SearchMode, Store,
};

const FIRST: &str = "discord:100000000000000001";
const SECOND: &str = "discord:100000000000000002";
const THIRD: &str = "discord:100000000000000003";

fn readers(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| (*id).to_owned()).collect()
}

fn private_memory(store: &mut Store, audience: &[&str], session: &str, wording: &str) -> i64 {
    let scope = store.register_private_audience(&readers(audience)).unwrap();
    store
        .record_evidence(EvidenceInput {
            session_id: session.into(),
            scope: scope.clone(),
            title: None,
            content: wording.into(),
            completed_at_ms: 100,
            metadata_json: "{}".into(),
        })
        .unwrap();
    store
        .distill_memory(DistillInput {
            // Every audience must be able to learn this key independently.
            canonical_key: "dinner:itinerary".into(),
            memory_kind: "fact".into(),
            scope,
            title: None,
            content: wording.into(),
            importance: 0.8,
            confidence: 0.9,
            pinned: false,
            evidence_session_id: session.into(),
            evidence_quote: wording.into(),
            supersedes: None,
            valid_until_ms: None,
        })
        .unwrap()
        .document_id
}

fn query(mode: SearchMode) -> ContextRequest {
    ContextRequest {
        query: "dinner itinerary".into(),
        mode,
        scope: None,
        limit: 8,
        max_chars: 8_000,
        evidence_per_memory: 2,
    }
}

#[test]
fn every_reader_must_have_seen_a_conversation_before_it_is_recalled() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("moon.sqlite");
    let mut store = Store::open(&path, 64).unwrap();
    let first = private_memory(
        &mut store,
        &[FIRST],
        "first",
        "Dinner itinerary includes a violin recital.",
    );
    let second = private_memory(
        &mut store,
        &[SECOND],
        "second",
        "Dinner itinerary includes a surprise cake.",
    );
    let shared = private_memory(
        &mut store,
        &[SECOND, FIRST],
        "both",
        "Dinner itinerary begins in the car park.",
    );
    let wording = "Dinner itinerary bookings need a reference number.";
    let published = store.publish_memory(FIRST, "fact", wording, None).unwrap()["document_id"]
        .as_i64()
        .unwrap();
    // A name or pin is not authority. Neither ordinary operator memories nor a
    // raw source imported under the public scope is an approved publication.
    for scope in ["global".to_string(), store.public_memory_scope().unwrap()] {
        store
            .remember(MemoryInput {
                memory_kind: "summary".into(),
                scope,
                title: None,
                content: "Dinner itinerary restricted pin.".into(),
                importance: 1.0,
                confidence: 1.0,
                pinned: true,
            })
            .unwrap();
    }
    store
        .ingest(IngestDocument {
            source_kind: "reference".into(),
            source_uri: "local://raw".into(),
            scope: store.public_memory_scope().unwrap(),
            title: None,
            content: "Dinner itinerary unpublished reference.".into(),
            modified_at_ms: 123,
            metadata_json: "{}".into(),
        })
        .unwrap();
    let provider = HashEmbedding::new(64);
    store.embed_pending(&provider, 64).unwrap();
    drop(store);
    let mut store = Store::open(&path, 64).unwrap();
    for mode in [
        SearchMode::Lexical,
        SearchMode::Hybrid,
        SearchMode::Semantic,
    ] {
        for (audience, expected) in [
            (vec![FIRST], vec![first, shared, published]),
            (vec![SECOND], vec![second, shared, published]),
            (vec![FIRST, SECOND], vec![shared, published]),
            (vec![THIRD, SECOND, FIRST], vec![published]),
            (vec![THIRD], vec![published]),
            (vec![], vec![published]),
        ] {
            let request = query(mode);
            let packet = store
                .observe_audience_context(&request, Some(&provider), &readers(&audience))
                .unwrap()
                .packet;
            let actual = packet
                .memories
                .iter()
                .map(|m| m.document_id)
                .collect::<BTreeSet<_>>();
            assert_eq!(
                actual,
                expected.into_iter().collect(),
                "{mode:?} {audience:?}"
            );
            assert!(packet.references.is_empty());
            assert!(packet.used_chars <= request.max_chars);
            for memory in &packet.memories {
                if memory.document_id == published {
                    assert!(memory.citations.is_empty());
                } else {
                    assert_eq!(memory.citations.len(), 1);
                    assert_eq!(memory.citations[0].quote, memory.content);
                }
            }
        }
    }
    let scope = store.register_private_audience(&readers(&[FIRST])).unwrap();
    let request = ContextRequest {
        scope: Some(scope),
        ..query(SearchMode::Lexical)
    };
    assert!(
        store
            .assemble_audience_context(&request, None, &readers(&[FIRST]))
            .is_err()
    );
    let exact = store.assemble_context(&request, None).unwrap();
    assert_eq!(
        exact
            .memories
            .iter()
            .map(|m| m.document_id)
            .collect::<Vec<_>>(),
        [first]
    );

    let connection = rusqlite::Connection::open(&path).unwrap();
    let uri: String = connection
        .query_row(
            "SELECT source_uri FROM documents WHERE id=?1",
            [published],
            |row| row.get(0),
        )
        .unwrap();
    let edit = IngestDocument {
        source_uri: uri,
        scope: store.public_memory_scope().unwrap(),
        source_kind: "memory".into(),
        title: None,
        content: "Unapproved replacement dinner itinerary.".into(),
        modified_at_ms: 234,
        metadata_json: "{}".into(),
    };
    assert!(store.ingest(edit.clone()).is_err());
    store.withdraw_publication(FIRST, published).unwrap();
    assert!(store.ingest(edit).is_err());
    assert!(
        store
            .assemble_audience_context(
                &query(SearchMode::Hybrid),
                Some(&provider),
                &readers(&[THIRD])
            )
            .unwrap()
            .is_empty()
    );
}

#[test]
fn matching_private_words_do_not_grant_group_access_or_expose_private_citations() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::open(temp.path().join("moon.sqlite"), 64).unwrap();
    let text = "The dinner itinerary includes a midnight ferry.";
    let first_dm = private_memory(&mut store, &[FIRST], "first-private", text);
    let second_dm = private_memory(&mut store, &[SECOND], "second-private", text);
    let provider = HashEmbedding::new(64);
    store.embed_pending(&provider, 64).unwrap();
    let group = readers(&[FIRST, SECOND]);
    let modes = [
        SearchMode::Lexical,
        SearchMode::Semantic,
        SearchMode::Hybrid,
    ];
    for mode in modes {
        for (destination, expected) in [
            (readers(&[FIRST]), vec![first_dm]),
            (readers(&[SECOND]), vec![second_dm]),
            (group.clone(), vec![]),
        ] {
            let packet = store
                .assemble_audience_context(&query(mode), Some(&provider), &destination)
                .unwrap();
            assert_eq!(
                packet
                    .memories
                    .iter()
                    .map(|m| m.document_id)
                    .collect::<Vec<_>>(),
                expected
            );
            assert!(packet.references.is_empty());
        }
    }

    let shared = private_memory(&mut store, &[FIRST, SECOND], "group-message", text);
    let public = store.publish_memory(FIRST, "fact", text, None).unwrap()["document_id"]
        .as_i64()
        .unwrap();
    store.embed_pending(&provider, 64).unwrap();
    for mode in modes {
        for (destination, allowed) in [
            (group.clone(), BTreeSet::from([shared, public])),
            (readers(&[FIRST, SECOND, THIRD]), BTreeSet::from([public])),
        ] {
            let packet = store
                .assemble_audience_context(&query(mode), Some(&provider), &destination)
                .unwrap();
            let found: BTreeSet<_> = packet.memories.iter().map(|m| m.document_id).collect();
            assert_eq!(found, allowed);
            assert_eq!(
                store.packet_is_public(&packet).unwrap(),
                allowed == BTreeSet::from([public])
            );
            for memory in packet.memories {
                assert_eq!(memory.content, text);
                if memory.document_id == shared {
                    assert_eq!(memory.citations.len(), 1);
                    assert_eq!(memory.citations[0].session_id, "group-message");
                } else {
                    assert!(memory.citations.is_empty());
                }
            }
        }
    }
}

#[test]
fn explicit_null_audience_cannot_select_the_operator_stdio_protocol() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::open(temp.path().join("moon.sqlite"), 64).unwrap();
    // New protocol requires a reader list and rejects null. The legacy opcode
    // also rejects an explicitly invalid list rather than treating it as absent.
    for (op, field) in [
        ("audience_context", ""),
        ("audience_context", r#","audience":null"#),
        ("context", r#","audience":null"#),
    ] {
        let line = format!(
            r#"{{"id":1,"op":"{op}","query":"itinerary","mode":"lexical","limit":4,"scope":null,"max_chars":2000,"evidence_per_memory":0{field}}}"#
        );
        let mut out = Vec::new();
        moon::server::serve_stdio(
            &mut store,
            &HashEmbedding::new(64),
            line.as_bytes(),
            &mut out,
        )
        .unwrap();
        let resp: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(resp["ok"], false, "{op} {field}");
    }
}
