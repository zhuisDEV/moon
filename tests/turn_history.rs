use moon::{EvidenceInput, Store, history::ReplyDelivery};
use tempfile::tempdir;

const A: &str = "discord:111111111111111111";
const B: &str = "discord:222222222222222222";
const C: &str = "discord:333333333333333333";

fn readers(who: &[&str]) -> Vec<String> {
    who.iter().map(|id| id.to_string()).collect()
}
fn record(store: &mut Store, key: &str, scope: &str, session: &str) {
    store
        .record_turn_evidence(
            EvidenceInput {
                session_id: session.into(),
                scope: scope.into(),
                title: None,
                content: "User:\nQuestion from the person.".into(),
                completed_at_ms: 1000,
                metadata_json: r#"{"source":"openclaw"}"#.into(),
            },
            Some(key),
        )
        .unwrap();
}

#[test]
fn confirmed_replies_only_recall_for_their_original_owners() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("history.sqlite");
    let mut store = Store::open(&path, 8).unwrap();
    let owners = readers(&[A, B]);
    let destination = readers(&[A, B, C]);
    let scope = store.register_private_audience(&owners).unwrap();
    let key = "a".repeat(64);
    store
        .bind_turn_owners(&key, &destination, A, &owners)
        .unwrap();
    let delivery = |id| {
        Some(ReplyDelivery {
            message_id: id,
            readers: &destination,
            owners: &owners,
        })
    };
    let first = "The answer was coral.";
    let mid = "999000000000000001";
    store
        .observe_private_reply(&key, first, delivery(mid))
        .unwrap();
    assert!(store.private_turn_history(&owners, 4).unwrap().is_empty());
    let db = rusqlite::Connection::open(&path).unwrap();
    let known = || {
        db.query_row(
            "SELECT count(*) FROM private_turn_history WHERE reply IS NOT NULL",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
    };
    assert_eq!(known(), 0);
    assert!(store.observe_private_reply(&key, first, None).is_err());
    record(&mut store, &key, &scope, "turn-1");
    store
        .observe_private_reply(&key, "An undelivered draft.", None)
        .unwrap();
    assert_eq!(known(), 0);
    drop(store);
    let mut store = Store::open(&path, 8).unwrap();
    store.observe_private_reply(&key, first, None).unwrap();
    store.observe_private_reply(&key, first, None).unwrap();
    store
        .observe_private_reply(&key, first, delivery(mid))
        .unwrap();
    assert_eq!(known(), 1);
    for who in [&owners, &readers(&[A]), &readers(&[B])] {
        let h = store.private_turn_history(who, 4).unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!(h[0]["role"], "user");
        assert_eq!(h[0]["text"], "Question from the person.");
        assert_eq!(h[1]["role"], "assistant");
        assert!(h[1].get("turn_sender").is_none());
        assert!(h[1]["delivered_at_ms"].is_number());
        assert_eq!(h[1]["text"], first);
    }
    for who in [destination.clone(), readers(&[C]), vec![]] {
        assert!(store.private_turn_history(&who, 4).unwrap().is_empty());
    }
    assert!(
        store
            .observe_private_reply(&key, "A different payload.", delivery(mid))
            .is_err()
    );
    let key2 = "b".repeat(64);
    store.bind_turn_owners(&key2, &owners, B, &owners).unwrap();
    record(&mut store, &key2, &scope, "turn-2");
    store
        .observe_private_reply(&key2, "A second reply.", None)
        .unwrap();
    assert_eq!(known(), 1);
    store
        .observe_private_reply(&key2, "A second reply.", delivery("999000000000000002"))
        .unwrap();
    assert_eq!(known(), 2);
    let cred = "My password is synthetic-rehearsal-credential";
    store.observe_private_reply(&key2, cred, None).unwrap();
    store
        .observe_private_reply(&key2, cred, delivery("999000000000000003"))
        .unwrap();
    assert_eq!(known(), 2);
    let claims: i64 = db
        .query_row("SELECT count(*) FROM memory_items", [], |r| r.get(0))
        .unwrap();
    assert_eq!(claims, 0);
}

#[test]
fn each_message_keeps_its_own_frozen_access_across_replay() {
    let dir = tempdir().unwrap();
    let mut store = Store::open(dir.path().join("room.sqlite"), 8).unwrap();
    let key = "9".repeat(64);
    let ask = readers(&[A, B]);
    let send = readers(&[B, C]);
    let live = readers(&[A, B, C]);
    store.bind_turn_owners(&key, &ask, A, &ask).unwrap();
    let scope = store.register_private_audience(&ask).unwrap();
    record(&mut store, &key, &scope, "conversation");
    let answer = "The answer has its own owner set.";
    let message = "999000000000000099";
    let receipt = |owners| {
        Some(ReplyDelivery {
            message_id: message,
            readers: &live,
            owners,
        })
    };
    store.observe_private_reply(&key, answer, None).unwrap();
    store
        .observe_private_reply(&key, answer, receipt(&send))
        .unwrap();
    assert!(
        store
            .observe_private_reply(&key, answer, receipt(&ask))
            .is_err()
    );
    let audit = store.evidence_by_sender(A, 0, 10).unwrap();
    assert_eq!(audit[0]["owners"], serde_json::json!(ask));
    assert_eq!(
        audit[0]["reply_deliveries"][0]["owners"],
        serde_json::json!(send)
    );
    assert_eq!(audit[0]["reply_deliveries"][0]["message_id"], message);
    assert!(audit[0]["question_access_at_ms"].is_i64());
    assert!(!audit[0].to_string().contains(answer));
    // The original speaker cannot recall the reply after losing history access.
    let a = store.private_turn_history(&readers(&[A]), 1).unwrap();
    assert_eq!(a.len(), 1);
    assert_eq!(a[0]["role"], "user");
    let b = store.private_turn_history(&readers(&[B]), 1).unwrap();
    assert_eq!(b.len(), 2);
    let c = store.private_turn_history(&readers(&[C]), 1).unwrap();
    assert_eq!(c.len(), 1);
    assert_eq!(c[0]["role"], "assistant");
    assert!(c[0].get("turn_sender").is_none());
    assert!(c[0].get("completed_at_ms").is_none());
    assert_eq!(store.private_turn_history(&ask, 1).unwrap(), a);
    assert_eq!(store.private_turn_history(&send, 1).unwrap(), c);
    assert!(store.private_turn_history(&live, 1).unwrap().is_empty());
}

#[test]
fn historic_deliveries_without_permissions_never_acquire_a_new_reader() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("legacy-history.sqlite");
    let mut store = Store::open(&db, 8).unwrap();
    let ask = readers(&[A]);
    let key = "f".repeat(64);
    store.bind_turn_owners(&key, &ask, A, &ask).unwrap();
    let scope = store.register_private_audience(&ask).unwrap();
    record(&mut store, &key, &scope, "legacy");
    let text = "Historic delivered text.";
    let mid = "999000000000000091";
    let raw = rusqlite::Connection::open(&db).unwrap();
    raw.execute("INSERT INTO private_turn_history(admission_key,reply_hash,accepted,discord_message_id,reply)
        VALUES(?1,?2,1,?3,?4)",rusqlite::params![key,moon::chunking::sha256_hex(text),mid,text]).unwrap();
    let receipt = ReplyDelivery {
        message_id: mid,
        readers: &ask,
        owners: &ask,
    };
    assert!(
        store
            .observe_private_reply(&key, text, Some(receipt))
            .is_err()
    );
    assert_eq!(store.private_turn_history(&ask, 1).unwrap().len(), 1);
    assert_eq!(
        raw.query_row("SELECT reply FROM private_turn_history", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        text
    );
}
