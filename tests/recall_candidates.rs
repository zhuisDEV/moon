use assert_cmd::Command;
use moon::recall::{RecallPacket, RecallQuery, RecallRequest};
use moon::{ContextRequest, DistillInput, EvidenceInput, HashEmbedding, SearchMode, Store};
use serde_json::{Value, json};
use std::path::Path;
use tempfile::TempDir;

const OWNER: &str = "discord:100000000000000001";
const OTHER: &str = "discord:100000000000000002";

fn setup() -> (TempDir, Store) {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(temp.path().join("state/moon.sqlite"), 64).unwrap();
    (temp, store)
}

fn learn(
    store: &mut Store,
    sender: &str,
    seq: u64,
    key: &str,
    wording: &str,
    source: (&str, &str),
    replaces: Option<i64>,
) -> i64 {
    let (original, quote) = source;
    let admission = format!("{seq:064x}");
    let owners = vec![sender.to_owned()];
    let session = format!("turn-{seq}");
    store
        .bind_turn_owners(&admission, &owners, sender, &owners)
        .unwrap();
    let scope = store.admitted_memory_scope(&admission, &session).unwrap();
    store
        .record_turn_evidence(
            EvidenceInput {
                session_id: session.clone(),
                scope: scope.clone(),
                title: None,
                content: format!(
                    "User:\n{original}\n\nAssistant:\nUnrelated assistant-only wording."
                ),
                completed_at_ms: seq as i64,
                metadata_json: "{}".into(),
            },
            Some(&admission),
        )
        .unwrap();
    store
        .distill_memory(DistillInput {
            canonical_key: key.into(),
            memory_kind: "fact".into(),
            scope,
            title: None,
            content: wording.into(),
            importance: 0.7,
            confidence: 0.9,
            pinned: false,
            evidence_session_id: session,
            evidence_quote: quote.into(),
            supersedes: replaces,
            valid_until_ms: None,
        })
        .unwrap()
        .document_id
}

fn request(audience: &[&str], queries: &[(&str, bool)]) -> RecallRequest {
    RecallRequest {
        queries: queries
            .iter()
            .map(|(text, temporal)| RecallQuery {
                text: (*text).into(),
                temporal: *temporal,
            })
            .collect(),
        audience: audience.iter().map(|id| (*id).into()).collect(),
        mode: SearchMode::Lexical,
        limit: 8,
        max_chars: 3500,
        allow_originals: true,
        observe: true,
    }
}

fn assert_budget(packet: &RecallPacket, request: &RecallRequest) {
    let json_size = serde_json::to_string(&packet.candidates)
        .unwrap()
        .chars()
        .count();
    assert_eq!(packet.packet_chars, json_size);
    assert!(json_size <= request.max_chars);
    assert_eq!(
        packet.memory_count,
        packet
            .candidates
            .iter()
            .map(|c| 1 + c.previous.len())
            .sum::<usize>()
    );
    assert!(packet.memory_count <= request.limit);
}

#[test]
fn temporal_projection_returns_exact_dated_speaker_evidence_only_under_its_source_grant() {
    let (_home, mut store) = setup();
    let first = learn(
        &mut store,
        OWNER,
        10,
        "notebook:label",
        "Notebook label is Fern.",
        (
            "I labelled the notebook Fern.",
            "I labelled the notebook Fern.",
        ),
        None,
    );
    let correction = learn(
        &mut store,
        OWNER,
        20,
        "notebook:label",
        "Notebook label is Indigo.",
        (
            "I never labelled it Fern. I meant Indigo from the beginning.",
            "I meant Indigo",
        ),
        Some(first),
    );
    let tool = learn(
        &mut store,
        OWNER,
        30,
        "drill:location",
        "Drill location is the shelf.",
        (
            "I keep the drill up on the shelf.",
            "the drill up on the shelf",
        ),
        None,
    );
    let query = request(&[OWNER], &[("notebook", true), ("drill", false)]);
    let packet = store.recall_candidates(&query, None).unwrap();
    assert_budget(&packet, &query);
    assert!(!packet.public_only);
    assert_eq!(packet.candidates.len(), 2);
    let a = &packet.candidates[0];
    assert_eq!(a.matched_queries, [0]);
    assert_eq!(a.claim.document_id, correction);
    assert_eq!(a.previous.len(), 1);
    assert_eq!(a.previous[0].document_id, first);
    assert_eq!(a.previous[0].citations[0].completed_at_ms, 10);
    let quote = &a.claim.citations[0];
    assert_eq!(quote.verified_sender, OWNER);
    assert_eq!(quote.completed_at_ms, 20);
    assert_eq!(quote.quote, "I meant Indigo");
    assert!(
        quote
            .speaker_excerpt
            .as_deref()
            .unwrap()
            .contains("never labelled it Fern")
    );
    let b = &packet.candidates[1];
    assert_eq!(b.claim.document_id, tool);
    assert_eq!(b.matched_queries, [1]);
    assert!(b.previous.is_empty() && b.claim.citations.is_empty());
    assert!(
        !serde_json::to_string(&packet)
            .unwrap()
            .contains("assistant-only")
    );

    for mut restricted in [
        request(&[OWNER], &[("notebook", false)]),
        RecallRequest {
            allow_originals: false,
            ..request(&[OWNER], &[("notebook", true)])
        },
    ] {
        restricted.limit = 2;
        let p = store.recall_candidates(&restricted, None).unwrap();
        assert_budget(&p, &restricted);
        assert_eq!(
            p.candidates[0].claim.verified_sender.as_deref(),
            Some(OWNER)
        );
        assert!(p.candidates[0].previous.is_empty() && p.candidates[0].claim.citations.is_empty());
        let json = serde_json::to_string(&p).unwrap();
        assert!(!json.contains("Fern") && !json.contains("from the beginning"));
    }
    for audience in [
        vec![OWNER, OTHER],
        vec![OTHER],
        vec![],
        vec![OWNER, "bot:discord:123"],
    ] {
        let packet = store
            .recall_candidates(&request(&audience, &[("notebook", true)]), None)
            .unwrap();
        assert!(packet.candidates.is_empty() && packet.public_only);
    }
}

#[test]
fn each_original_and_supersession_edge_must_be_eligible_including_hidden_sources() {
    let (home, mut store) = setup();
    let first = learn(
        &mut store,
        OWNER,
        1,
        "tent:site",
        "Tent was pitched at the beach.",
        ("My old tent stood at the beach.", "tent stood at the beach"),
        None,
    );
    let second = learn(
        &mut store,
        OWNER,
        2,
        "tent:site",
        "Tent is pitched at the summit.",
        (
            "I moved the tent to the summit.",
            "moved the tent to the summit",
        ),
        Some(first),
    );
    let hidden = learn(
        &mut store,
        OTHER,
        3,
        "tent:site",
        "Private tent is in the marsh.",
        ("I hid my tent in the marsh.", "tent in the marsh"),
        None,
    );
    let query = request(&[OWNER], &[("tent", true)]);
    let p = store.recall_candidates(&query, None).unwrap();
    assert_eq!(
        p.candidates[0].previous[0].citations[0].quote,
        "tent stood at the beach"
    );

    // An extra citation outside the quote limit must still veto the entire
    // revision. Same canonical key, foreign speaker and different source ACL.
    let db = rusqlite::Connection::open(home.path().join("state/moon.sqlite")).unwrap();
    db.execute("INSERT INTO memory_citations(memory_document_id,evidence_session_id,evidence_document_id,
                  start_byte,end_byte,start_line,end_line,quote,created_at_ms)
                SELECT ?1,evidence_session_id,evidence_document_id,start_byte,end_byte,start_line,end_line,quote,0
                FROM memory_citations WHERE memory_document_id=?2", [first,hidden]).unwrap();
    assert!(
        db.execute(
            "UPDATE memory_items SET superseded_by=?1 WHERE document_id=?2",
            [second, hidden],
        )
        .is_err()
    );
    let p = store.recall_candidates(&query, None).unwrap();
    assert_eq!(p.candidates.len(), 1);
    assert!(p.candidates[0].previous.is_empty());
    let rendered = serde_json::to_string(&p).unwrap();
    assert!(!rendered.contains("marsh") && !rendered.contains("beach"));
    // A forged dependency on a nonexistent delivered reply invalidates even
    // the current saved wording, including on the facts-only read.
    db.execute("UPDATE memory_citations SET prior_reply='{\"message_id\":\"999\",\"quote\":\"fabricated\"}'
                WHERE memory_document_id=?1", [second]).unwrap();
    let p = store
        .recall_candidates(&request(&[OWNER], &[("tent", false)]), None)
        .unwrap();
    assert!(p.candidates.is_empty());
}

#[test]
fn bounded_semantic_candidates_reach_the_judge_without_a_final_word_overlap_test() {
    let (_home, mut store) = setup();
    let phrase = "The vespine journal bears the label Indigo.";
    let id = learn(
        &mut store,
        OWNER,
        1,
        "journal:label",
        phrase,
        (phrase, phrase),
        None,
    );
    let provider = HashEmbedding::new(64);
    store.embed_pending(&provider, 100).unwrap();
    // The deterministic local provider proves control flow, not language ability.
    let mut req = request(&[OWNER], &[("herbier de collection", false)]);
    for mode in [SearchMode::Semantic, SearchMode::Hybrid] {
        req.mode = mode;
        let old = store
            .assemble_audience_context(
                &ContextRequest {
                    query: req.queries[0].text.clone(),
                    scope: None,
                    mode,
                    limit: 8,
                    evidence_per_memory: 0,
                    max_chars: 3500,
                },
                Some(&provider),
                &req.audience,
            )
            .unwrap();
        assert!(old.is_empty());
        let packet = store.recall_candidates(&req, Some(&provider)).unwrap();
        assert_eq!(packet.candidates[0].claim.document_id, id);
        assert_eq!(packet.candidates[0].matched_queries, [0]);
    }
}

#[test]
fn hybrid_tail_hits_and_mixed_best_rank_ties_do_not_upgrade_source_access() {
    let (_home, mut store) = setup();
    for (n, topic, fact) in [
        (1, "notebook", "Notebook label is Fern."),
        (2, "drill", "Drill location is the shelf."),
    ] {
        learn(&mut store, OWNER, n, topic, fact, (fact, fact), None);
    }
    let provider = HashEmbedding::new(64);
    store.embed_pending(&provider, 100).unwrap();
    let mut q = request(&[OWNER], &[("notebook", true), ("drill", false)]);
    q.mode = SearchMode::Hybrid;
    let p = store.recall_candidates(&q, Some(&provider)).unwrap();
    assert_eq!(p.candidates.len(), 2);
    for c in &p.candidates {
        assert_eq!(c.matched_queries, [0, 1]); // Both turn up in each vector ranking.
        let notebook = c.claim.content.starts_with("Notebook");
        assert_eq!(c.temporal, notebook);
        assert_eq!(!c.claim.citations.is_empty(), notebook);
    }
    q.queries[1].text = "notebook label".into();
    let p = store.recall_candidates(&q, Some(&provider)).unwrap();
    let notebook = p
        .candidates
        .iter()
        .find(|c| c.claim.content.starts_with("Notebook"))
        .unwrap();
    assert_eq!(notebook.matched_queries, [0, 1]);
    assert!(!notebook.temporal);
    assert!(notebook.claim.citations.is_empty() && notebook.previous.is_empty());
}

#[test]
fn bounded_revision_chain_and_legacy_owner_never_broaden_into_operator_or_public_history() {
    let (home, mut store) = setup();
    let mut last = None;
    let mut ids = Vec::new();
    for (n, name) in ["Crane", "Egret", "Heron", "Ibis"].iter().enumerate() {
        let wording = format!("The telescope now carries the label {name}.");
        let id = learn(
            &mut store,
            OWNER,
            (n + 1) as u64,
            "telescope",
            &wording,
            (&wording, &wording),
            last,
        );
        ids.push(id);
        last = Some(id);
    }
    let mut q = request(&[OWNER], &[("telescope", true)]);
    for (limit, expected) in [(8, vec![ids[2], ids[1]]), (2, vec![ids[2]]), (1, vec![])] {
        q.limit = limit;
        let packet = store.recall_candidates(&q, None).unwrap();
        assert_budget(&packet, &q);
        assert!(packet.truncated);
        assert_eq!(packet.candidates[0].claim.document_id, ids[3]);
        assert_eq!(
            packet.candidates[0]
                .previous
                .iter()
                .map(|p| p.document_id)
                .collect::<Vec<_>>(),
            expected
        );
        assert!(!serde_json::to_string(&packet).unwrap().contains("Crane"));
    }

    let wording = "The telescope archive uses a linen shelf.";
    let legacy = store
        .remember(moon::MemoryInput {
            memory_kind: "fact".into(),
            scope: "global".into(),
            title: None,
            content: wording.into(),
            importance: 0.8,
            confidence: 0.9,
            pinned: false,
        })
        .unwrap()
        .document_id;
    let db = rusqlite::Connection::open(home.path().join("state/moon.sqlite")).unwrap();
    db.execute(
        "INSERT INTO privacy_legacy_documents(document_id) VALUES(?1)",
        [legacy],
    )
    .unwrap();
    let archive = request(&[OWNER], &[("linen", true)]);
    assert!(
        store
            .recall_candidates(&archive, None)
            .unwrap()
            .candidates
            .is_empty()
    );
    store.assign_legacy_collection(OWNER).unwrap();
    let packet = store.recall_candidates(&archive, None).unwrap();
    assert!(!packet.public_only);
    let saved = &packet.candidates[0];
    assert_eq!(saved.claim.content, wording);
    assert!(saved.claim.verified_sender.is_none() && saved.claim.observed_at_ms.is_none());
    assert!(saved.previous.is_empty() && saved.claim.citations.is_empty());
    let published = store
        .publish_memory(OWNER, "fact", "The linen archive is open.", None)
        .unwrap()["document_id"]
        .as_i64()
        .unwrap();
    let shared = request(&[OWNER, OTHER], &[("linen", true)]);
    let p = store.recall_candidates(&shared, None).unwrap();
    assert!(p.public_only);
    assert_eq!(p.candidates.len(), 1);
    assert_eq!(p.candidates[0].claim.document_id, published);
    store.withdraw_publication(OWNER, published).unwrap();
    assert!(
        store
            .recall_candidates(&shared, None)
            .unwrap()
            .candidates
            .is_empty()
    );
}

#[test]
fn round_robin_deduplicates_subqueries_and_enforces_one_complete_unicode_budget() {
    let (_home, mut store) = setup();
    for wording in [
        "Orchid first.",
        "Orchid second.",
        "Bromeliad speaks \"你好\".",
    ] {
        store.publish_memory(OWNER, "fact", wording, None).unwrap();
    }
    let mut q = request(
        &[],
        &[("orchid", true), ("bromeliad", true), ("orchid", false)],
    );
    q.limit = 2;
    let p = store.recall_candidates(&q, None).unwrap();
    assert_budget(&p, &q);
    assert!(p.public_only && p.truncated);
    assert_eq!(p.candidates[0].matched_queries, [0, 2]);
    assert_eq!(p.candidates[1].matched_queries, [1]);
    assert!(p.candidates[1].claim.content.contains("你好"));
    for c in &p.candidates {
        assert!(c.claim.verified_sender.is_none() && c.claim.observed_at_ms.is_none());
        assert!(c.previous.is_empty() && c.claim.citations.is_empty());
    }
    let long = "Orchid ".to_owned() + &"你好\n\"".repeat(85);
    store.publish_memory(OWNER, "fact", &long, None).unwrap();
    q.max_chars = 512;
    q.limit = 8;
    let limited = store.recall_candidates(&q, None).unwrap();
    assert_budget(&limited, &q);
    assert!(limited.truncated);
    assert!(limited.candidates.iter().all(|c| c.claim.content != long));
    assert_eq!(
        serde_json::to_value(&limited.candidates).unwrap(),
        serde_json::to_value(&store.recall_candidates(&q, None).unwrap().candidates).unwrap(),
    );
}

fn moon(home: &Path) -> Command {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("moon"));
    cmd.env_remove("MOON_DATABASE")
        .env_remove("MOON_HOME")
        .arg("--home")
        .arg(home)
        .args(["--dimensions", "64", "--json"]);
    cmd
}

#[test]
fn cli_and_warm_worker_require_an_explicit_audience_and_return_identical_candidates() {
    let (home, mut store) = setup();
    let text = "That titanium flask belongs to the steward.";
    let id = learn(
        &mut store,
        OWNER,
        1,
        "flask:owner",
        text,
        (text, text),
        None,
    );
    let args = ["recall-candidates", "--mode", "lexical"];
    let queries = json!([{"text":"titanium flask","temporal":true}]);
    let readers = json!([OWNER]).to_string();
    moon(home.path())
        .args(args)
        .write_stdin(queries.to_string())
        .assert()
        .failure();
    let result = moon(home.path())
        .args(args)
        .args(["--audience-json", &readers])
        .write_stdin(queries.to_string())
        .assert()
        .success();
    let cli: Value = serde_json::from_slice(&result.get_output().stdout).unwrap();
    assert_eq!(cli["candidates"][0]["document_id"], id);
    assert_eq!(cli["candidates"][0]["citations"], json!([]));
    let key = cli["request_id"].as_str().unwrap();
    store.mark_context_injected(key, false).unwrap();

    let base = json!({"id":1,"op":"recall_candidates", "queries":queries,"mode":"lexical",
                      "limit":8,"max_chars":3500,"audience":[OWNER]});
    let mut absent = base.clone();
    absent.as_object_mut().unwrap().remove("audience");
    let mut null = base.clone();
    null["audience"] = Value::Null;
    let mut operator = base.clone();
    operator["scope"] = json!("global");
    let mut wide = base.clone();
    wide["queries"] = json!([queries[0], queries[0], queries[0], queries[0]]);
    let mut with_sources = base.clone();
    with_sources["allow_originals"] = json!(true);
    let mut wrong = base.clone();
    wrong["audience"] = json!([OWNER, OTHER]);
    let frames = [absent, null, operator, wide, wrong, base, with_sources]
        .into_iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let out = moon(home.path())
        .args(["serve", "--provider", "hash"])
        .write_stdin(frames)
        .assert()
        .success();
    let rows: Vec<Value> = std::str::from_utf8(&out.get_output().stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    for failure in &rows[..4] {
        assert_eq!(failure["ok"], false);
    }
    assert_eq!(rows[4]["result"]["candidates"], json!([]));
    assert_eq!(rows[5]["result"]["candidates"], cli["candidates"]);
    assert_eq!(
        rows[6]["result"]["candidates"][0]["citations"][0]["quote"],
        text
    );
}
