use moon::source_learning::{SourcePrepareRequest, SourceProposalInput};
use moon::sources::SourceInput;
use moon::topics::{TopicInput, TopicPrepareRequest, TopicProposalInput};
use moon::{ContextRequest, HashEmbedding, SearchMode, SearchRequest, Store};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::Path;

const SCOPE: &str = "retrieval-validity";
const QUOTE: &str =
    "Retrieval canary retains complete SQLite evidence and its final qualification.";

fn text<'a>(value: &'a Value, field: &str) -> &'a str {
    value[field].as_str().unwrap()
}

fn source(key: &str, quote: &str) -> SourceInput {
    serde_json::from_value(json!({
        "schema_version":1,"source_key":key,"kind":"external_document",
        "author":format!("Author {key}"),"content":quote
    }))
    .unwrap()
}

fn learn(store: &mut Store, scope: &str, key: &str, quote: &str) -> String {
    learn_with_expiry(store, scope, key, quote, None)
}

fn learn_with_expiry(
    store: &mut Store,
    scope: &str,
    key: &str,
    quote: &str,
    expiry: Option<i64>,
) -> String {
    store
        .source_import(scope, source(key, quote), None, true)
        .unwrap();
    let prepared = store
        .source_learning_prepare(&SourcePrepareRequest {
            scope: scope.into(),
            key: Some(key.into()),
            max_bytes: 65536,
            lease_ms: 60000,
            preview: false,
        })
        .unwrap();
    let proposal: SourceProposalInput = serde_json::from_value(
        json!({"schema_version":1,"claims":[{"quote":quote,"valid_until_ms":expiry}]}),
    )
    .unwrap();
    let review = store
        .source_learning_submit(
            scope,
            text(&prepared, "job_id"),
            text(&prepared, "lease_token"),
            proposal,
        )
        .unwrap();
    store
        .source_learning_accept(
            scope,
            text(&prepared, "job_id"),
            text(&review, "result_hash"),
        )
        .unwrap()["claim_ids"][0]
        .as_str()
        .unwrap()
        .to_owned()
}

fn compile_topic(store: &mut Store, claim: &str) {
    compile_topic_in(store, SCOPE, "retention", claim);
}

fn compile_topic_in(store: &mut Store, scope: &str, key: &str, claim: &str) {
    let definition: TopicInput = serde_json::from_value(json!({
        "schema_version":1,"key":key,"title":"Retrieval canary SQLite evidence",
        "claim_ids":[claim],"topic_ids":[]
    }))
    .unwrap();
    store.topic_put(scope, definition, None).unwrap();
    let prepared = store
        .topic_prepare(&TopicPrepareRequest {
            scope: scope.into(),
            key: Some(key.into()),
            max_bytes: 65536,
            lease_ms: 60000,
            preview: false,
        })
        .unwrap();
    let proposal: TopicProposalInput = serde_json::from_value(json!({
        "schema_version":1,"review_complete":true,
        "statements":[{"kind":"fact","text":QUOTE,"input_ids":[claim]}]
    }))
    .unwrap();
    let review = store
        .topic_submit(
            scope,
            text(&prepared, "job_id"),
            text(&prepared, "lease_token"),
            proposal,
        )
        .unwrap();
    store
        .topic_accept(
            scope,
            text(&prepared, "job_id"),
            text(&review, "result_hash"),
        )
        .unwrap();
}

fn rows(connection: &Connection, query: &str) -> Vec<(String, i64)> {
    connection
        .prepare(query)
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

fn old_view() -> &'static str {
    let migration = include_str!("../migrations/0010_source_learning.sql");
    let start = migration
        .find("CREATE VIEW source_valid_claims AS")
        .unwrap();
    let end = migration[start..].find(';').unwrap() + start + 1;
    &migration[start..end]
}

#[test]
fn validity_rewrite_preserves_unique_support_expiry_revision_and_scope_rules() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::open(temp.path().join("state/moon.sqlite"), 64).unwrap();
    let shared = learn(&mut store, SCOPE, "shared-a", QUOTE);
    assert_eq!(shared, learn(&mut store, SCOPE, "shared-b", QUOTE));
    let foreign = learn(&mut store, "other-scope", "shared-a", QUOTE);
    assert_ne!(shared, foreign);
    let withdrawn = learn(
        &mut store,
        SCOPE,
        "withdrawn",
        "Retrieval canary withdrawn evidence.",
    );
    let state = store.source_state(SCOPE, "withdrawn").unwrap();
    store
        .source_set_withdrawn(
            SCOPE,
            "withdrawn",
            text(&state, "revision_id"),
            state["generation"].as_i64().unwrap(),
            true,
            "Synthetic lifecycle fixture",
        )
        .unwrap();
    let historical = learn(
        &mut store,
        SCOPE,
        "revised",
        "Retrieval canary historical evidence.",
    );
    let prior = store.source_get(SCOPE, "revised", None).unwrap();
    store
        .source_import(
            SCOPE,
            source(
                "revised",
                "Retrieval canary corrected source has not yet been accepted.",
            ),
            Some(text(&prior, "revision_id")),
            true,
        )
        .unwrap();
    let expired = learn(
        &mut store,
        SCOPE,
        "expired",
        "Retrieval canary expiring evidence.",
    );
    let unsupported = learn(
        &mut store,
        SCOPE,
        "unsupported",
        "Retrieval canary unsupported evidence.",
    );
    let mismatched = learn(
        &mut store,
        SCOPE,
        "mismatched",
        "Retrieval canary mismatched evidence.",
    );
    let ineligible = learn(
        &mut store,
        SCOPE,
        "ineligible",
        "Retrieval canary ineligible evidence.",
    );
    let provider = HashEmbedding::new(64);
    store.embed_pending(&provider, 100).unwrap();

    // Simulate elapsed time and stale/corrupt index state without maintenance.
    // These direct mutations are confined to the disposable test database.
    let connection = Connection::open(store.path()).unwrap();
    connection.execute_batch("DROP TRIGGER source_claims_immutable_update; DROP TRIGGER sources_immutable_update; DROP TRIGGER source_revisions_immutable_update;").unwrap();
    connection
        .execute(
            "UPDATE source_claims SET valid_until_ms=1 WHERE claim_id=?1",
            [&expired],
        )
        .unwrap();
    connection
        .execute(
            "DELETE FROM source_claim_supports WHERE claim_id=?1",
            [&unsupported],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE sources SET scope='other-scope' WHERE source_key='mismatched'",
            [],
        )
        .unwrap();
    connection.execute("UPDATE source_revisions SET learning_eligibility='reference_only' WHERE source_id=(SELECT source_id FROM sources WHERE source_key='ineligible')", []).unwrap();
    assert!(connection.query_row("SELECT d.active FROM documents d JOIN source_claims c ON c.document_id=d.id WHERE c.claim_id=?1", [&expired], |row| row.get::<_,bool>(0)).unwrap());

    let original_select = old_view()
        .strip_prefix("CREATE VIEW source_valid_claims AS\n")
        .unwrap()
        .trim_end_matches(';');
    let original = rows(
        &connection,
        &format!("{original_select} ORDER BY c.claim_id"),
    );
    let current = rows(
        &connection,
        "SELECT claim_id,document_id FROM source_valid_claims ORDER BY claim_id",
    );
    assert_eq!(current, original);
    assert_eq!(
        current.len(),
        2,
        "multiple supports must not duplicate a claim"
    );
    assert_eq!(
        current
            .iter()
            .map(|(id, _)| id.clone())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([shared.clone(), foreign.clone()])
    );
    for claim in [
        withdrawn,
        historical,
        expired,
        unsupported,
        mismatched,
        ineligible,
    ] {
        assert_eq!(
            store.source_claim_get(SCOPE, &claim).unwrap()["active"],
            false
        );
    }
    for scope in [SCOPE, "other-scope"] {
        let expected = if scope == SCOPE { &shared } else { &foreign };
        let detail = store.source_claim_get(scope, expected).unwrap();
        for mode in [
            SearchMode::Lexical,
            SearchMode::Semantic,
            SearchMode::Hybrid,
        ] {
            let hits = store
                .search(
                    &SearchRequest {
                        query: "Retrieval canary evidence".into(),
                        mode,
                        limit: 1,
                        scope: Some(scope.into()),
                        source_kind: Some("source_claim".into()),
                    },
                    Some(&provider),
                )
                .unwrap();
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].document_id, detail["document_id"].as_i64().unwrap());
            assert_eq!(hits[0].scope, scope);
        }
    }
}

#[test]
fn warmed_queries_rebind_scope_kind_and_limit_and_see_expiry_from_another_connection() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::open(temp.path().join("state/moon.sqlite"), 64).unwrap();
    let expiring = learn_with_expiry(
        &mut store,
        SCOPE,
        "expiring",
        QUOTE,
        Some(chrono::Utc::now().timestamp_millis() + 86_400_000),
    );
    let stable = learn(&mut store, SCOPE, "stable", QUOTE);
    let foreign = learn(&mut store, "other-scope", "stable", QUOTE);
    assert_ne!(expiring, stable);
    compile_topic_in(&mut store, SCOPE, "expiring", &expiring);
    compile_topic_in(&mut store, SCOPE, "stable", &stable);
    compile_topic_in(&mut store, "other-scope", "stable", &foreign);
    let provider = HashEmbedding::new(64);
    store.embed_pending(&provider, 100).unwrap();

    let writer = Connection::open(store.path()).unwrap();
    // Do DDL before warming the reader. The later DML must be observed without
    // recompiling prepared statements because of a schema change.
    writer
        .execute_batch("DROP TRIGGER source_claims_immutable_update;")
        .unwrap();
    let schema_cookie: i64 = writer
        .query_row("PRAGMA schema_version", [], |row| row.get(0))
        .unwrap();
    let expired_document: i64 = writer
        .query_row(
            "SELECT document_id FROM source_claims WHERE claim_id=?1",
            [&expiring],
            |row| row.get(0),
        )
        .unwrap();
    let expired_topic: i64 = writer.query_row("SELECT r.document_id FROM topic_revisions r JOIN topics t ON t.topic_id=r.topic_id WHERE t.scope=?1 AND t.topic_key='expiring'", [SCOPE], |row| row.get(0)).unwrap();
    let excluded = BTreeSet::from([expired_document, expired_topic]);
    let documents: Vec<(i64, String, String)> = writer
        .prepare("SELECT id,scope,source_kind FROM documents ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(documents.len(), 6);
    let mut requests = Vec::new();
    for mode in [
        SearchMode::Lexical,
        SearchMode::Semantic,
        SearchMode::Hybrid,
    ] {
        for (scope, kind, limit) in [
            (Some(SCOPE), Some("source_claim"), 1),
            (Some("other-scope"), Some("source_claim"), 8),
            (None, Some("source_claim"), 8),
            (Some(SCOPE), Some("topic_summary"), 8),
            (Some("other-scope"), Some("topic_summary"), 1),
            (None, Some("topic_summary"), 8),
            (None, None, 8),
            (Some(SCOPE), None, 8),
            (Some(SCOPE), Some("source_claim"), 8),
        ] {
            requests.push(SearchRequest {
                query: "Retrieval canary SQLite evidence".into(),
                mode,
                limit,
                scope: scope.map(str::to_owned),
                source_kind: kind.map(str::to_owned),
            });
        }
    }
    let inspect = |expired: bool| {
        for request in &requests {
            let expected = documents
                .iter()
                .filter(|(id, scope, kind)| {
                    (!expired || !excluded.contains(id))
                        && request.scope.as_ref().is_none_or(|wanted| wanted == scope)
                        && request
                            .source_kind
                            .as_ref()
                            .is_none_or(|wanted| wanted == kind)
                })
                .map(|(id, _, _)| *id)
                .collect::<BTreeSet<_>>();
            let hits = store.search(request, Some(&provider)).unwrap();
            assert_eq!(hits.len(), expected.len().min(request.limit), "{request:?}");
            let returned = hits
                .iter()
                .map(|hit| hit.document_id)
                .collect::<BTreeSet<_>>();
            assert_eq!(returned.len(), hits.len());
            assert!(returned.is_subset(&expected), "{request:?}");
        }
        for scope in [Some(SCOPE), Some("other-scope"), None, Some(SCOPE)] {
            let packet = store
                .assemble_context(
                    &ContextRequest {
                        scope: scope.map(str::to_owned),
                        ..context_request()
                    },
                    None,
                )
                .unwrap();
            assert!(!packet.references.is_empty());
            for reference in packet.references {
                assert!(scope.is_none_or(|wanted| reference.scope == wanted));
                assert!(!expired || !excluded.contains(&reference.document_id));
                if let Some(claim) = reference.source_claim {
                    assert!(!expired || claim.claim_id != expiring);
                    assert!(
                        claim
                            .citations
                            .iter()
                            .all(|citation| citation.quote == QUOTE)
                    );
                }
            }
        }
    };
    inspect(false);
    inspect(false);
    writer
        .execute(
            "UPDATE source_claims SET valid_until_ms=1 WHERE claim_id=?1",
            [&expiring],
        )
        .unwrap();
    assert_eq!(
        writer
            .query_row("PRAGMA schema_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        schema_cookie
    );
    assert!(
        writer
            .query_row(
                "SELECT active FROM documents WHERE id=?1",
                [expired_document],
                |row| row.get::<_, bool>(0)
            )
            .unwrap()
    );
    inspect(true);
    inspect(true);
}

fn schema_eleven_fixture(path: &Path) -> (String, Value, Value, Value) {
    let mut store = Store::open(path, 64).unwrap();
    let claim = learn(&mut store, SCOPE, "retained", QUOTE);
    assert_eq!(claim, learn(&mut store, SCOPE, "independent", QUOTE));
    compile_topic(&mut store, &claim);
    let claim_before = store.source_claim_get(SCOPE, &claim).unwrap();
    let topic_before = store.topic_get(SCOPE, "retention").unwrap();
    let packet_before =
        serde_json::to_value(store.assemble_context(&context_request(), None).unwrap()).unwrap();
    drop(store);
    // Remove later additive diagnostics and restore migration 10's view so this
    // remains a genuine schema-11 fixture as the current schema advances.
    let connection = Connection::open(path).unwrap();
    connection.execute_batch("ALTER TABLE source_learning_jobs DROP COLUMN failure_code; ALTER TABLE topic_jobs DROP COLUMN failure_code;").unwrap();
    connection
        .execute_batch("DROP VIEW source_valid_claims;")
        .unwrap();
    connection.execute_batch(old_view()).unwrap();
    connection
        .execute("DELETE FROM schema_migrations WHERE version>11", [])
        .unwrap();
    connection.pragma_update(None, "user_version", 11).unwrap();
    (claim, claim_before, topic_before, packet_before)
}

fn context_request() -> ContextRequest {
    ContextRequest {
        query: "Retrieval canary SQLite evidence qualification".into(),
        mode: SearchMode::Lexical,
        limit: 8,
        scope: Some(SCOPE.into()),
        max_chars: 8000,
        evidence_per_memory: 2,
    }
}

fn view_sql(connection: &Connection) -> String {
    connection
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name='source_valid_claims'",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn schema_eleven_migration_preserves_source_topic_and_packet_then_reopens() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state/moon.sqlite");
    let (claim, claim_before, topic_before, packet_before) = schema_eleven_fixture(&path);
    let store = Store::open(&path, 64).unwrap();
    assert_eq!(store.health().unwrap().schema_version, 13);
    assert!(store.health().unwrap().ok);
    assert_eq!(store.source_claim_get(SCOPE, &claim).unwrap(), claim_before);
    assert_eq!(store.topic_get(SCOPE, "retention").unwrap(), topic_before);
    assert_eq!(
        serde_json::to_value(store.assemble_context(&context_request(), None).unwrap()).unwrap(),
        packet_before
    );
    drop(store);
    let reopened = Store::open_existing(&path, 64).unwrap();
    assert_eq!(
        reopened.source_claim_get(SCOPE, &claim).unwrap(),
        claim_before
    );
    assert_eq!(
        reopened.topic_get(SCOPE, "retention").unwrap(),
        topic_before
    );
}

#[test]
fn failed_schema_twelve_ledger_commit_restores_original_view_and_data() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state/moon.sqlite");
    let (claim, claim_before, topic_before, packet_before) = schema_eleven_fixture(&path);
    let connection = Connection::open(&path).unwrap();
    let original_view = view_sql(&connection);
    let original_rows = rows(
        &connection,
        "SELECT claim_id,document_id FROM source_valid_claims ORDER BY claim_id",
    );
    connection.execute_batch("CREATE TRIGGER reject_migration_twelve BEFORE INSERT ON schema_migrations WHEN NEW.version=12 BEGIN SELECT RAISE(ABORT,'synthetic migration failure'); END;").unwrap();
    drop(connection);
    assert!(Store::open(&path, 64).is_err());
    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        11
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM schema_migrations WHERE version=12",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(view_sql(&connection), original_view);
    assert_eq!(
        rows(
            &connection,
            "SELECT claim_id,document_id FROM source_valid_claims ORDER BY claim_id"
        ),
        original_rows
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT content FROM source_claims WHERE claim_id=?1",
                params![&claim],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        QUOTE
    );
    connection
        .execute_batch("DROP TRIGGER reject_migration_twelve;")
        .unwrap();
    drop(connection);
    let store = Store::open(&path, 64).unwrap();
    assert_eq!(store.source_claim_get(SCOPE, &claim).unwrap(), claim_before);
    assert_eq!(store.topic_get(SCOPE, "retention").unwrap(), topic_before);
    assert_eq!(
        serde_json::to_value(store.assemble_context(&context_request(), None).unwrap()).unwrap(),
        packet_before
    );
    assert!(store.health().unwrap().ok);
}
