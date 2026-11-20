//! A room's readers own the conversation. Each claim still has its own source speaker.
use moon::{
    ContextRequest, DistillInput, EvidenceInput, SearchMode, Store,
    learning::{ApplyInput, PrepareRequest},
};
use serde_json::json;

const FIRST: &str = "discord:100000000000000011";
const SECOND: &str = "discord:100000000000000022";
const OUTSIDER: &str = "discord:100000000000000033";
const QUOTE: &str = "I prefer peppermint tea with breakfast.";
const CLAIM: &str = "The speaker prefers peppermint tea with breakfast.";
const KEY: &str = "user:breakfast:tea";

fn record(store: &mut Store, speaker: &str, n: u64, quote: &str) -> (String, String) {
    let key = format!("{n:064x}");
    let session = format!("turn-{n}");
    let readers = [FIRST.to_owned(), SECOND.to_owned()];
    store
        .bind_turn_owners(&key, &readers, speaker, &readers)
        .unwrap();
    let scope = store.admitted_memory_scope(&key, &session).unwrap();
    store
        .record_turn_evidence(
            EvidenceInput {
                session_id: session.clone(),
                scope: scope.clone(),
                title: None,
                content: format!("User:\n{quote}"),
                completed_at_ms: n as i64,
                metadata_json: "{}".into(),
            },
            Some(&key),
        )
        .unwrap();
    (scope, session)
}

fn proposal(turn: &(String, String), key: &str) -> DistillInput {
    DistillInput {
        scope: turn.0.clone(),
        evidence_session_id: turn.1.clone(),
        canonical_key: key.into(),
        memory_kind: "preference".into(),
        title: None,
        content: CLAIM.into(),
        importance: 0.9,
        confidence: 0.95,
        pinned: false,
        evidence_quote: QUOTE.into(),
        supersedes: None,
        valid_until_ms: None,
    }
}

fn recall(store: &mut Store, audience: &[&str], evidence_per_memory: usize) -> moon::ContextPacket {
    store
        .assemble_audience_context(
            &ContextRequest {
                query: "peppermint tea breakfast".into(),
                mode: SearchMode::Lexical,
                scope: None,
                limit: 8,
                max_chars: 16000,
                evidence_per_memory,
            },
            None,
            &audience.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        )
        .unwrap()
}

#[test]
fn shared_room_preserves_two_identical_preferences_and_their_speakers() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("moon.sqlite");
    let mut store = Store::open(&path, 64).unwrap();
    let first = record(&mut store, FIRST, 1, QUOTE);
    let second = record(&mut store, SECOND, 2, QUOTE);
    assert_ne!(first.0, second.0);
    let first_id = store
        .distill_memory(proposal(&first, KEY))
        .unwrap()
        .document_id;
    let second_id = store
        .distill_memory(proposal(&second, KEY))
        .unwrap()
        .document_id;
    assert_ne!(first_id, second_id);
    // An independent, equal-text confirmation reuses only this speaker's head,
    // even when the model proposes an alternative key for both people.
    for (speaker, n, id) in [(FIRST, 3, first_id), (SECOND, 4, second_id)] {
        let repeat = record(&mut store, speaker, n, QUOTE);
        assert_eq!(
            store
                .distill_memory(proposal(&repeat, "morning:tea"))
                .unwrap()
                .document_id,
            id
        );
    }
    drop(store);
    let mut store = Store::open(&path, 64).unwrap();
    for evidence_limit in [0, 2] {
        for room in [vec![FIRST, SECOND], vec![FIRST], vec![SECOND]] {
            let packet = recall(&mut store, &room, evidence_limit);
            assert_eq!(packet.memories.len(), 2);
            let rendered = packet.render_markdown();
            for (id, speaker) in [(first_id, FIRST), (second_id, SECOND)] {
                let m = packet
                    .memories
                    .iter()
                    .find(|m| m.document_id == id)
                    .unwrap();
                assert_eq!(m.source_speakers, [speaker]);
                assert!(rendered.contains(speaker));
                assert_eq!(m.citations.len(), evidence_limit);
                assert!(
                    m.citations
                        .iter()
                        .all(|c| c.verified_sender.as_deref() == Some(speaker))
                );
            }
        }
        assert!(recall(&mut store, &[FIRST, SECOND, OUTSIDER], evidence_limit).is_empty());
    }
}

#[test]
fn another_speakers_record_cannot_be_a_correction_merge_or_confirmation_target() {
    let home = tempfile::tempdir().unwrap();
    let mut store = Store::open(home.path().join("moon.sqlite"), 64).unwrap();
    let first = record(&mut store, FIRST, 1, QUOTE);
    let second = record(&mut store, SECOND, 2, QUOTE);
    let own = store
        .distill_memory(proposal(&first, KEY))
        .unwrap()
        .document_id;
    let foreign = store
        .distill_memory(proposal(&second, KEY))
        .unwrap()
        .document_id;
    let quote = "I now prefer chamomile tea with breakfast.";
    let correction = record(&mut store, FIRST, 3, quote);
    let change = DistillInput {
        content: quote.into(),
        evidence_quote: quote.into(),
        supersedes: Some(foreign),
        ..proposal(&correction, KEY)
    };
    assert!(store.distill_memory(change).is_err());
    let prepared = store
        .learning_prepare(&PrepareRequest {
            run_key: "source-test".into(),
            rule_version: "speaker-policy".into(),
            review_session: None,
            occurrence: None,
            limit: 16,
            max_chars: 32000,
            lease_ms: 60000,
            max_attempts: 3,
            before_ms: None,
            after_ms: None,
            preview: false,
        })
        .unwrap();
    assert_eq!(prepared["scope"], first.0);
    let evidence = prepared["evidence"].as_array().unwrap();
    assert!(!evidence.is_empty());
    assert!(evidence.iter().all(|e| e["verified_sender"] == FIRST));
    let run = prepared["run_id"].as_str().unwrap();
    for (action, target, others) in [
        ("confirm", foreign, vec![]),
        ("supersede", foreign, vec![]),
        ("review", foreign, vec![]),
        ("merge", own, vec![foreign]),
    ] {
        let input: ApplyInput=serde_json::from_value(json!({
            "actions":[{"action":action,"canonical_key":KEY,"kind":"preference","content":CLAIM,
              "confidence":0.95,"importance":0.9,"target_document_id":target,"merge_document_ids":others,
              "evidence":[{"session_id":first.1,"quote":QUOTE}]}]
        })).unwrap();
        assert!(
            store.learning_apply(run, input, false).is_err(),
            "accepted cross-speaker {action}"
        );
    }
    let valid:ApplyInput=serde_json::from_value(json!({
        "actions":[{"action":"confirm","canonical_key":KEY,"kind":"preference","content":CLAIM,
        "confidence":0.95,"importance":0.9,"target_document_id":own,"evidence":[{"session_id":first.1,"quote":QUOTE}]}]
    })).unwrap();
    store.learning_apply(run, valid, false).unwrap();
    assert_eq!(recall(&mut store, &[FIRST, SECOND], 0).memories.len(), 2);
}

#[test]
fn expired_speaker_head_is_hidden_from_recall_and_eligible_for_correction() {
    let home = tempfile::tempdir().unwrap();
    let mut store = Store::open(home.path().join("moon.sqlite"), 64).unwrap();
    let original = record(&mut store, FIRST, 1, QUOTE);
    let old = store
        .distill_memory(DistillInput {
            valid_until_ms: Some(2),
            ..proposal(&original, KEY)
        })
        .unwrap();
    assert!(recall(&mut store, &[FIRST], 1).is_empty());

    let update = "I now prefer chamomile tea with breakfast.";
    let correction = record(&mut store, FIRST, 3, update);
    let prepared = store
        .learning_prepare(&PrepareRequest {
            run_key: "correct-expired".into(),
            rule_version: String::new(),
            occurrence: None,
            review_session: None,
            limit: 8,
            max_chars: 32_000,
            lease_ms: 60_000,
            max_attempts: 3,
            before_ms: None,
            after_ms: Some(3),
            preview: false,
        })
        .unwrap();
    assert_eq!(prepared["memories"][0]["document_id"], old.document_id);
    assert!(
        prepared["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["session_id"] == original.1
                && e["selected"] == false
                && e["verified_sender"] == FIRST)
    );
    let action = serde_json::from_value(json!({"actions":[{
        "action":"supersede","canonical_key":KEY,"kind":"preference","content":update,
        "confidence":0.95,"importance":0.9,"target_document_id":old.document_id,
        "evidence":[{"session_id":correction.1,"quote":update}]
    }]}))
    .unwrap();
    let applied = store
        .learning_apply(prepared["run_id"].as_str().unwrap(), action, false)
        .unwrap();
    assert_eq!(
        applied["outcomes"][0]["superseded_document_id"],
        old.document_id
    );
    assert!(recall(&mut store, &[OUTSIDER], 1).is_empty());
}

#[test]
fn knowing_the_owner_of_an_unattributed_private_claim_does_not_supply_a_speaker() {
    let home = tempfile::tempdir().unwrap();
    let mut store = Store::open(home.path().join("moon.sqlite"), 64).unwrap();
    let scope = store
        .register_private_audience(&[FIRST.to_owned()])
        .unwrap();
    let session = "unattributed-operator-import".to_owned();
    store
        .record_evidence(EvidenceInput {
            session_id: session.clone(),
            scope: scope.clone(),
            title: None,
            content: QUOTE.into(),
            completed_at_ms: 1,
            metadata_json: "{}".into(),
        })
        .unwrap();
    store
        .distill_memory(proposal(&(scope, session), KEY))
        .unwrap();
    for evidence_limit in [0, 1] {
        let packet = recall(&mut store, &[FIRST], evidence_limit);
        assert_eq!(packet.memories.len(), 1);
        let memory = &packet.memories[0];
        assert!(memory.source_speakers.is_empty());
        assert!(memory.source_speaker_unverified);
        assert!(memory.citations.iter().all(|c| c.verified_sender.is_none()));
    }
}

#[test]
fn external_bot_has_its_own_provenance_and_only_master_can_recall_it() {
    const BOT: &str = "bot:discord:100000000000000044";
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("moon.toml"),
        format!("[master]\nprofile_id='person-one'\nmemory_principal='{FIRST}'\n"),
    )
    .unwrap();
    let master = moon::config::load(home.path()).unwrap().master.unwrap();
    let mut store = Store::open(home.path().join("moon.sqlite"), 64).unwrap();
    let key = "b".repeat(64);
    let session = "bot:fixture";
    let readers = vec![FIRST.to_owned(), SECOND.to_owned(), BOT.to_owned()];
    assert!(
        store
            .bind_bot_turn(&key, &readers[1..], BOT, &master)
            .is_err()
    );
    assert!(
        store
            .bind_bot_turn(&key, &readers[..2], BOT, &master)
            .is_err()
    );
    assert!(
        store
            .bind_bot_turn(&key, &readers, SECOND, &master)
            .is_err()
    );
    assert!(
        store
            .bind_turn_owners(&key, &readers, BOT, &readers)
            .is_err()
    );
    assert!(store.register_private_audience(&[BOT.to_owned()]).is_err());
    for _ in 0..2 {
        assert_eq!(
            store.bind_bot_turn(&key, &readers, BOT, &master).unwrap(),
            [FIRST]
        );
    }
    assert!(
        store
            .bind_turn_owners(&key, &readers, FIRST, &[FIRST.into()])
            .is_err()
    );
    let bot = store.turn_admission(&key).unwrap();
    assert_eq!(bot.sender.as_deref(), Some(BOT));
    assert_eq!(bot.sender_kind.as_deref(), Some("bot"));
    let bot_scope = store.admitted_memory_scope(&key, session).unwrap();
    let evidence = |role| EvidenceInput {
        session_id: session.to_owned(),
        scope: bot_scope.clone(),
        title: None,
        content: format!("{role}:\n{QUOTE}"),
        completed_at_ms: 1,
        metadata_json: "{}".into(),
    };
    assert!(
        store
            .record_turn_evidence(evidence("User"), Some(&key))
            .is_err()
    );
    assert!(
        store
            .record_turn_evidence(evidence("Bot"), Some(&key))
            .unwrap()
            .changed
    );
    assert!(
        !store
            .record_turn_evidence(evidence("Bot"), Some(&key))
            .unwrap()
            .changed
    );
    store
        .distill_memory(proposal(&(bot_scope.clone(), session.to_owned()), KEY))
        .unwrap();
    let admitted_human = store
        .bind_turn_owners(&"c".repeat(64), &readers, FIRST, &[FIRST.to_owned()])
        .unwrap();
    assert_eq!(admitted_human, [FIRST]);
    assert_ne!(
        store
            .admitted_memory_scope(&"c".repeat(64), "human")
            .unwrap(),
        bot_scope
    );
    let private = recall(&mut store, &[FIRST], 1);
    assert_eq!(private.memories.len(), 1);
    assert_eq!(private.memories[0].source_speakers, [BOT]);
    assert_eq!(
        private.memories[0].citations[0].verified_sender.as_deref(),
        Some(BOT)
    );
    for viewers in [
        vec![SECOND],
        vec![BOT],
        vec![FIRST, BOT],
        vec![FIRST, SECOND],
    ] {
        assert!(recall(&mut store, &viewers, 0).is_empty());
    }
    let history = store.private_turn_history(&[FIRST.into()], 3).unwrap();
    assert_eq!(history[0]["role"], "bot");
    assert_eq!(history[0]["turn_sender"], BOT);
    assert_eq!(history[0]["text"], QUOTE);
    assert!(
        store
            .private_turn_history(&[SECOND.into()], 3)
            .unwrap()
            .is_empty()
    );
    let batch = store
        .learning_prepare(&PrepareRequest {
            run_key: "bot-test".into(),
            rule_version: "bot-rule".into(),
            review_session: None,
            occurrence: None,
            limit: 5,
            max_chars: 16000,
            lease_ms: 60000,
            max_attempts: 3,
            before_ms: None,
            after_ms: None,
            preview: true,
        })
        .unwrap();
    assert_eq!(batch["evidence"][0]["verified_sender"], BOT);
    assert!(
        batch["evidence"][0]["content"]
            .as_str()
            .unwrap()
            .starts_with("Bot:\n")
    );
    // The same verified bot can correct its own report; the resulting claim
    // retains the private scope and its source speaker.
    let corrected_key = "d".repeat(64);
    let corrected_session = "bot:correction";
    let corrected = "Mint.";
    store
        .bind_bot_turn(&corrected_key, &readers, BOT, &master)
        .unwrap();
    store
        .record_turn_evidence(
            EvidenceInput {
                session_id: corrected_session.into(),
                scope: bot_scope.clone(),
                title: None,
                content: format!("Bot:\n{corrected}"),
                completed_at_ms: 2,
                metadata_json: "{}".into(),
            },
            Some(&corrected_key),
        )
        .unwrap();
    let updated = store
        .distill_memory(DistillInput {
            supersedes: Some(private.memories[0].document_id),
            evidence_quote: corrected.into(),
            content: corrected.into(),
            ..proposal(&(bot_scope, corrected_session.into()), KEY)
        })
        .unwrap();
    assert_eq!(
        updated.superseded_document_id,
        Some(private.memories[0].document_id)
    );
    assert!(recall(&mut store, &[FIRST, SECOND], 1).is_empty());
    assert!(store.health().unwrap().ok);
}
