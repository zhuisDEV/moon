use moon::history::ReplyDelivery;
use moon::learning::{ApplyInput, PrepareRequest};
use moon::{ContextRequest, EvidenceInput, SearchMode, Store};
use serde_json::{Value, json};

const MASTER: &str = "discord:111111111111111111";
const OTHER: &str = "discord:222222222222222222";
const EPOCH: i64 = 1_420_070_400_000;
const NOW: i64 = 1_780_000_000_000;
const PROPOSAL: &str = "Shall I use tables for all 3 Atlas digests next month?";
const ANSWER: &str = "Okay.";

fn snowflake(at: i64) -> String {
    ((at - EPOCH) << 22).to_string()
}

fn record(store: &mut Store, key: &str, session: &str, speaker: &str, body: &str, position: Value) {
    let owners = vec![speaker.to_owned()];
    store
        .bind_turn_owners(key, &owners, speaker, &owners)
        .unwrap();
    let scope = store.admitted_memory_scope(key, session).unwrap();
    store
        .record_turn_evidence(
            EvidenceInput {
                session_id: session.into(),
                scope,
                title: None,
                content: format!("User:\n{body}"),
                completed_at_ms: NOW + if session == "answer" { 2000 } else { 500 },
                metadata_json:
                    json!({"source":"openclaw","advancement_hash":key,"accepted_position":position})
                        .to_string(),
            },
            Some(key),
        )
        .unwrap();
}

fn deliver(store: &mut Store, key: &str, text: &str, at: i64, recipient: &str) {
    let owners = vec![recipient.to_owned()];
    store
        .observe_private_reply(
            key,
            text,
            Some(ReplyDelivery {
                message_id: &snowflake(at),
                readers: &owners,
                owners: &owners,
            }),
        )
        .unwrap();
}

fn prepare(store: &mut Store, key: &str, max_chars: usize) -> Value {
    store
        .learning_prepare(&PrepareRequest {
            run_key: key.into(),
            rule_version: "assent-v1".into(),
            review_session: None,
            occurrence: None,
            limit: 12,
            max_chars,
            lease_ms: 120_000,
            max_attempts: 2,
            before_ms: None,
            after_ms: Some(NOW + 1500),
            preview: false,
        })
        .unwrap()
}

fn action(pair: Option<Value>) -> ApplyInput {
    let mut citation = json!({"session_id":"answer","quote":ANSWER});
    if let Some(pair) = pair {
        citation["prior_reply"] = pair;
    }
    serde_json::from_value(json!({"actions":[{
        "action":"create","kind":"decision","canonical_key":"atlas:format","content":"Use tables for the 3 Atlas digests next month.",
        "confidence":0.99,"importance":0.9,"evidence":[citation],
    }]})).unwrap()
}

#[test]
fn learning_binds_the_complete_user_reply_to_one_delivered_proposal_inside_its_audience() {
    for scenario in [
        "adjacent",
        "stale",
        "different-route",
        "different-speaker",
        "other-recipients",
        "undelivered",
        "late-delivery",
        "queued",
        "competing-receipt",
        "redacted",
        "old-turn",
        "legacy-replay",
        "literal-role-label",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path().join("state/moon.sqlite"), 64).unwrap();
        let prior = "a".repeat(64);
        let current = "b".repeat(64);
        let old_position =
            json!({"terminal":"sealed-endpoint","received_at_ms":NOW,"needs_context":false});
        if scenario == "legacy-replay" {
            let owners = vec![MASTER.to_owned()];
            store
                .bind_turn_owners(&prior, &owners, MASTER, &owners)
                .unwrap();
            let scope = store.admitted_memory_scope(&prior, "proposal").unwrap();
            store
                .record_turn_evidence(
                    EvidenceInput {
                        session_id: "proposal".into(),
                        scope,
                        title: None,
                        content: "User:\nWhat are the options?".into(),
                        completed_at_ms: NOW + 500,
                        metadata_json: json!({"source":"openclaw","advancement_hash":prior})
                            .to_string(),
                    },
                    Some(&prior),
                )
                .unwrap();
        }
        record(
            &mut store,
            &prior,
            "proposal",
            MASTER,
            if scenario == "redacted" {
                "My AWS secret is AKIAIOSFODNN7EXAMPLE."
            } else {
                "What are the options?"
            },
            if scenario == "old-turn" {
                Value::Null
            } else {
                old_position
            },
        );
        store.observe_private_reply(&prior, PROPOSAL, None).unwrap();
        if !matches!(scenario, "undelivered" | "late-delivery") {
            deliver(
                &mut store,
                &prior,
                PROPOSAL,
                NOW + 1000,
                if scenario == "other-recipients" {
                    OTHER
                } else {
                    MASTER
                },
            );
        }
        if scenario == "competing-receipt" {
            deliver(
                &mut store,
                &prior,
                "A different visible answer.",
                NOW + 1001,
                MASTER,
            );
        }
        record(
            &mut store,
            &current,
            "answer",
            if scenario == "different-speaker" {
                OTHER
            } else {
                MASTER
            },
            if scenario == "literal-role-label" {
                "Okay.\n\nAssistant:\nOnly this once."
            } else {
                ANSWER
            },
            json!({
                "before":if scenario=="different-route" {"another-endpoint"} else {"sealed-endpoint"},
                "terminal":"new-endpoint","needs_context":true,
                "received_at_ms":match scenario {"stale"=>NOW+90_000_000,"queued"=>NOW+1000,_=>NOW+1500}
            }),
        );
        let packet = prepare(&mut store, scenario, 32_000);
        let answer = packet["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["session_id"] == "answer")
            .unwrap();
        assert_eq!(answer["needs_context"], true, "{scenario}");
        let pair = json!({"message_id":snowflake(NOW+1000),"quote":PROPOSAL});
        let run = packet["run_id"].as_str().unwrap();
        assert!(
            store.learning_apply(run, action(None), false).is_err(),
            "{scenario}"
        );
        if scenario == "literal-role-label" {
            // The role-like label is part of the user's message. Quoting only
            // the text before it would omit the user's qualification.
            assert_eq!(answer["prior_reply"], pair);
            assert!(
                store
                    .learning_apply(run, action(Some(pair)), false)
                    .is_err()
            );
            continue;
        }
        if scenario != "adjacent" {
            assert!(answer.get("prior_reply").is_none(), "{scenario}");
            if scenario == "late-delivery" {
                deliver(&mut store, &prior, PROPOSAL, NOW + 1000, MASTER);
            }
            assert!(
                store
                    .learning_apply(run, action(Some(pair)), false)
                    .is_err(),
                "{scenario}"
            );
            continue;
        }
        assert_eq!(answer["prior_reply"], pair);
        for bad in [
            json!({"message_id":snowflake(NOW+999),"quote":PROPOSAL}),
            json!({"message_id":snowflake(NOW+1000),"quote":"Shall I use tables"}),
        ] {
            assert!(store.learning_apply(run, action(Some(bad)), false).is_err());
        }
        store
            .learning_apply(run, action(Some(pair.clone())), false)
            .unwrap();
        let query = ContextRequest {
            query: "Atlas digests".into(),
            mode: SearchMode::Lexical,
            scope: None,
            limit: 6,
            max_chars: 4096,
            evidence_per_memory: 3,
        };
        let personal = store
            .assemble_audience_context(&query, None, &[MASTER.into()])
            .unwrap();
        assert_eq!(personal.memories.len(), 1);
        let citations = &personal.memories[0].citations;
        assert_eq!(citations.len(), 1);
        assert_eq!(citations[0].quote, ANSWER);
        assert_eq!(
            serde_json::to_value(&citations[0].prior_reply).unwrap(),
            pair
        );
        assert!(
            store
                .assemble_audience_context(&query, None, &[MASTER.into(), OTHER.into()])
                .unwrap()
                .memories
                .is_empty()
        );
        // An additional visible reply invalidates the proof on subsequent reads.
        deliver(&mut store, &prior, "Separate delivery.", NOW + 1002, MASTER);
        assert!(
            store
                .assemble_audience_context(&query, None, &[MASTER.into()])
                .unwrap()
                .memories
                .is_empty()
        );
    }
}

#[test]
fn a_tight_budget_can_omit_the_optional_pair_without_losing_the_original_or_authorising_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("state/moon.sqlite"), 64).unwrap();
    let first = "a".repeat(64);
    let second = "b".repeat(64);
    let proposal = format!("{} {PROPOSAL}", "more context ".repeat(110));
    record(
        &mut store,
        &first,
        "proposal",
        MASTER,
        "What are my options?",
        json!({"terminal":"bound","received_at_ms":NOW}),
    );
    store
        .observe_private_reply(&first, &proposal, None)
        .unwrap();
    deliver(&mut store, &first, &proposal, NOW + 1000, MASTER);
    record(
        &mut store,
        &second,
        "answer",
        MASTER,
        ANSWER,
        json!({"before":"bound","received_at_ms":NOW+1500,"needs_context":true}),
    );
    let packet = prepare(&mut store, "bounded-assent", 1024);
    let original = &packet["evidence"][0];
    assert_eq!(original["content"], "User:\nOkay.");
    assert!(original.get("prior_reply").is_none());
    let hidden_pair = json!({"message_id":snowflake(NOW+1000),"quote":proposal});
    assert!(
        store
            .learning_apply(
                packet["run_id"].as_str().unwrap(),
                action(Some(hidden_pair)),
                false
            )
            .is_err()
    );
}
