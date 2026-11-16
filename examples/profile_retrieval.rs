//! Profile SQL against retained synthetic packet-benchmark snapshots only.
//! cargo run --release --example profile_retrieval --
//!   target/packet-runtime-seeds/10000.sqlite [candidate-migration.sql]
//! The snapshot is copied into a temporary directory; no Moon home is opened.
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, StatementStatus, params_from_iter, types::Value};
use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};
use std::{path::Path, time::Instant};

const SCOPE: &str = "synthetic-packet-runtime";
const ROUNDS: usize = 20;

fn profile(connection: &Connection, name: &str, sql: &str, values: Vec<Value>) -> Result<Json> {
    let plan = connection
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?
        .query_map(params_from_iter(&values), |row| row.get::<_, String>(3))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut statement = connection.prepare(sql)?;
    let mut samples = Vec::new();
    let mut returned_rows = 0;
    for round in 0..ROUNDS + 3 {
        for status in [
            StatementStatus::VmStep,
            StatementStatus::FullscanStep,
            StatementStatus::Sort,
        ] {
            statement.reset_status(status);
        }
        let start = Instant::now();
        let mut rows = statement.query(params_from_iter(&values))?;
        let mut count = 0;
        while rows.next()?.is_some() {
            count += 1;
        }
        drop(rows);
        if round >= 3 {
            returned_rows = count;
            samples.push(json!({
                "elapsed_ms": start.elapsed().as_secs_f64() * 1000.0,
                "vm_steps": statement.get_status(StatementStatus::VmStep),
                "fullscan_steps": statement.get_status(StatementStatus::FullscanStep),
                "sorts": statement.get_status(StatementStatus::Sort),
            }));
        }
    }
    let mut prepare_and_step_ms = Vec::new();
    for round in 0..ROUNDS + 3 {
        let start = Instant::now();
        let mut freshly_prepared = connection.prepare(sql)?;
        let mut rows = freshly_prepared.query(params_from_iter(&values))?;
        let mut count = 0;
        while rows.next()?.is_some() {
            count += 1;
        }
        ensure!(
            count == returned_rows,
            "Statement preparation changed row count"
        );
        drop(rows);
        drop(freshly_prepared);
        if round >= 3 {
            prepare_and_step_ms.push(start.elapsed().as_secs_f64() * 1000.0);
        }
    }
    let mut fingerprint = Sha256::new();
    let columns = statement.column_count();
    let mut rows = statement.query(params_from_iter(&values))?;
    while let Some(row) = rows.next()? {
        for column in 0..columns {
            let value: Value = row.get(column)?;
            fingerprint.update(format!("{value:?}\n").as_bytes());
        }
        fingerprint.update(b"row-end\n");
    }
    drop(rows);
    Ok(
        json!({"name":name,"sql":sql,"query_plan":plan,"returned_rows":returned_rows,"result_sha256":hex::encode(fingerprint.finalize()),"samples":samples,"prepare_and_step_ms":prepare_and_step_ms}),
    )
}

// Exercise the actual per-hit lookup across cache returns and changed bindings.
// A retained statement with one repeated parameter misses SQLite 3.54's
// reprepare cost when rusqlite clears bindings as it returns a statement.
fn cached_lexical_metadata(connection: &Connection) -> Result<Json> {
    let original = r#"SELECT d.id, c.id, d.source_uri, d.source_kind, d.scope, d.title, c.content
             FROM chunks c
             JOIN documents d ON d.id = c.document_id
             WHERE c.id = ?1
               AND d.active = 1
               AND (d.source_kind <> 'source_claim' OR EXISTS (
                   SELECT 1 FROM source_valid_claims sc WHERE sc.document_id = d.id
               ))
               AND (d.source_kind <> 'topic_summary' OR EXISTS (
                   SELECT 1 FROM topic_valid_documents tv WHERE tv.document_id = d.id
               ))
               AND (
                   NOT EXISTS (
                       SELECT 1 FROM memory_items m
                       WHERE m.document_id = d.id
                   )
                   OR EXISTS (
                       SELECT 1 FROM memory_items m
                       WHERE m.document_id = d.id
                         AND m.superseded_by IS NULL
                         AND (m.valid_until_ms IS NULL OR m.valid_until_ms > ?4)
                   )
               )
               AND (?2 IS NULL OR d.scope = ?2)
               AND (?3 IS NULL OR d.source_kind = ?3)
               AND (?5 = 0 OR d.source_kind <> 'topic_summary')"#;
    let cast = original.replace("c.id = ?1", "c.id = CAST(?1 AS INTEGER)");
    ensure!(
        original != cast,
        "Metadata diagnostic must cast the chunk ID"
    );
    let mut ids = connection.prepare(
        "SELECT min(c.id) FROM chunks c JOIN documents d ON d.id=c.document_id GROUP BY d.source_kind ORDER BY d.source_kind",
    )?.query_map([], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ids.extend([0, -1, i64::MAX]);
    let mut parameters = Vec::new();
    for id in ids {
        for (scope, kind, exclude, now) in [
            (SCOPE, Value::Null, false, 2_000_000_000_000i64),
            (
                SCOPE,
                Value::Text("source_claim".into()),
                true,
                9_000_000_000_000_000i64,
            ),
            (
                "unrelated-synthetic-scope",
                Value::Null,
                false,
                2_000_000_000_000i64,
            ),
        ] {
            parameters.push(vec![
                id.into(),
                scope.to_string().into(),
                kind,
                now.into(),
                exclude.into(),
            ]);
        }
    }
    let mut samples = [Vec::new(), Vec::new()];
    let mut fingerprint = Sha256::new();
    let mut parity_checks = 0;
    for round in 0..ROUNDS + 3 {
        for (case, values) in parameters.iter().enumerate() {
            let mut outputs = [Vec::new(), Vec::new()];
            // Alternate order so one form does not always warm the other's pages.
            for variant in if (round + case) % 2 == 0 {
                [0, 1]
            } else {
                [1, 0]
            } {
                let started = Instant::now();
                let mut statement =
                    connection.prepare_cached(if variant == 0 { original } else { &cast })?;
                statement.reset_status(StatementStatus::RePrepare);
                let columns = statement.column_count();
                let rows = statement
                    .query_map(params_from_iter(values), |row| {
                        (0..columns)
                            .map(|column| row.get::<_, Value>(column))
                            .collect::<rusqlite::Result<Vec<_>>>()
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
                if round >= 3 {
                    samples[variant].push(json!({
                        "case":case,"elapsed_ms":elapsed_ms,
                        "reprepares":statement.get_status(StatementStatus::RePrepare),
                        "returned_rows":rows.len(),
                    }));
                }
                outputs[variant] = rows;
                // Drop returns this statement to the cache and clears bindings.
            }
            ensure!(
                outputs[0] == outputs[1],
                "Chunk-ID cast changed metadata rows or column values"
            );
            if round >= 3 {
                fingerprint.update(format!("{case}:{:?}\n", outputs[0]).as_bytes());
                parity_checks += 1;
            }
        }
    }
    Ok(json!({
        "name":"cached_lexical_metadata_changing_integer_ids",
        "original_sql":original,"candidate_sql":cast,"parameter_cases":parameters.len(),
        "full_row_parity_checks":parity_checks,
        "result_sha256":hex::encode(fingerprint.finalize()),
        "original_samples":samples[0],"cast_samples":samples[1],
        "limitations":["Every chunk ID is bound as an i64; casting arbitrary external text is not covered.","Cache-return and prepare costs are included; this is not complete packet latency."]
    }))
}

fn run(snapshot: &Path, migration: Option<&str>) -> Result<Json> {
    let temporary = tempfile::Builder::new()
        .prefix("retrieval-sql-profile-")
        .tempdir_in("target")?;
    let database = temporary.path().join("synthetic.sqlite");
    std::fs::copy(snapshot, &database)?;
    let connection = Connection::open(&database)?;
    let outside_scope: i64 = connection.query_row(
        "SELECT count(*) FROM documents WHERE scope<>?1",
        [SCOPE],
        |row| row.get(0),
    )?;
    ensure!(
        outside_scope == 0,
        "Only the synthetic packet corpus is allowed"
    );
    let document_count: i64 =
        connection.query_row("SELECT count(*) FROM documents", [], |row| row.get(0))?;
    ensure!(
        matches!(document_count, 1004 | 10004),
        "Unexpected synthetic corpus size"
    );
    let claim_document: i64 = connection.query_row(
        "SELECT document_id FROM source_claims ORDER BY created_at_ms DESC,claim_id LIMIT 1",
        [],
        |row| row.get(0),
    )?;
    let topic_document: i64 = connection.query_row(
        "SELECT document_id FROM topic_revisions WHERE redacted=0 LIMIT 1",
        [],
        |row| row.get(0),
    )?;
    let claim_id: String = connection.query_row(
        "SELECT claim_id FROM source_claims WHERE document_id=?1",
        [claim_document],
        |row| row.get(0),
    )?;
    let chunk_id: i64 = connection.query_row(
        "SELECT id FROM chunks WHERE document_id=?1 LIMIT 1",
        [claim_document],
        |row| row.get(0),
    )?;
    let premise_values = connection.query_row(
        "SELECT s.source_id,x.revision_id,x.start_byte,x.end_byte,c.content,s.scope,r.revision_id,r.document_id
         FROM topic_revisions r JOIN topic_dependencies dep ON dep.job_id=r.job_id AND dep.kind='claim'
         JOIN source_claims c ON c.claim_id=dep.input_id
         JOIN source_claim_supports x ON x.claim_id=c.claim_id
         JOIN source_revisions sr ON sr.revision_id=x.revision_id
         JOIN sources s ON s.source_id=sr.source_id WHERE r.document_id=?1 LIMIT 1",
        [topic_document], |row| (0..8).map(|i| row.get::<_,Value>(i)).collect::<rusqlite::Result<Vec<_>>>(),
    )?;
    if let Some(sql) = migration {
        // An arbitrary migration is accepted only after verifying the synthetic
        // copy; errors roll back, and the retained seed is never opened for SQL.
        connection.execute_batch("BEGIN IMMEDIATE")?;
        connection.execute_batch(sql)?;
        connection.execute_batch("COMMIT")?;
    }
    let mut measurements = vec![
        profile(
            &connection,
            "source_validity_by_document",
            "SELECT 1 FROM source_valid_claims WHERE document_id=?1",
            vec![claim_document.into()],
        )?,
        profile(
            &connection,
            "source_claim_context_identity",
            "SELECT c.claim_id,c.kind FROM source_claims c JOIN source_valid_claims v ON v.claim_id=c.claim_id WHERE c.document_id=?1",
            vec![claim_document.into()],
        )?,
        profile(
            &connection,
            "topic_validity_by_document",
            "SELECT 1 FROM topic_valid_documents WHERE document_id=?1",
            vec![topic_document.into()],
        )?,
        profile(
            &connection,
            "exact_topic_premise",
            "SELECT d.id,c.id FROM source_valid_supports s
         JOIN source_valid_claims v ON v.claim_id=s.claim_id
         JOIN documents d ON d.id=v.document_id JOIN chunks c ON c.document_id=d.id
         WHERE s.source_id=?1 AND s.revision_id=?2 AND s.start_byte=?3 AND s.end_byte=?4
           AND d.body=?5 AND d.scope=?6 AND d.active=1 AND d.source_kind='source_claim'
           AND EXISTS(SELECT 1 FROM topic_valid_documents WHERE revision_id=?7 AND document_id=?8)
         ORDER BY c.id LIMIT 1",
            premise_values,
        )?,
    ];
    measurements.push(profile(&connection,"reference_metadata",
        "SELECT d.body,c.start_byte FROM chunks c JOIN documents d ON d.id=c.document_id
         WHERE c.id=?1 AND c.document_id=?2 AND d.active=1
         AND (d.source_kind<>'source_claim' OR EXISTS(SELECT 1 FROM source_valid_claims sc WHERE sc.document_id=d.id))
         AND (d.source_kind<>'topic_summary' OR EXISTS(SELECT 1 FROM topic_valid_documents tv WHERE tv.document_id=d.id))",
        vec![chunk_id.into(),claim_document.into()])?);
    measurements.push(profile(&connection,"source_citations",
        "SELECT v.source_id,v.source_key,v.revision_id,r.kind,r.origin,r.title,r.author,r.published_at_ms,r.captured_at_ms,v.start_byte,v.end_byte,v.start_line,v.end_line,r.content
         FROM source_valid_supports v JOIN source_revisions r ON r.revision_id=v.revision_id
         WHERE v.claim_id=?1 ORDER BY r.captured_at_ms DESC,v.source_id,v.start_byte LIMIT ?2",
        vec![claim_id.into(),3.into()])?);
    measurements.push(profile(&connection,"topic_context",
        "SELECT r.topic_id,t.topic_key,r.revision_id,r.content_json FROM topic_valid_documents v
         JOIN topic_revisions r ON r.revision_id=v.revision_id JOIN topics t ON t.topic_id=r.topic_id WHERE v.document_id=?1",
        vec![topic_document.into()])?);
    measurements.push(profile(
        &connection,
        "fuzzy_memories",
        "SELECT d.id,c.id,d.source_uri,d.source_kind,d.scope,d.title,c.content FROM chunks c
         JOIN documents d ON d.id=c.document_id JOIN memory_items m ON m.document_id=d.id
         WHERE d.active=1 AND d.source_kind='memory' AND m.superseded_by IS NULL
         AND (m.valid_until_ms IS NULL OR m.valid_until_ms>?1) AND (?2 IS NULL OR d.scope=?2)
         ORDER BY m.pinned DESC,m.importance DESC,m.last_confirmed_at_ms DESC,c.id LIMIT 2000",
        vec![2_000_000_000_000i64.into(), SCOPE.to_string().into()],
    )?);
    let lexical_sql = "SELECT chunk_fts.rowid,bm25(chunk_fts,4.0,1.0,0.0,0.0) AS lexical_score
         FROM chunk_fts LEFT JOIN chunks c ON c.id=chunk_fts.rowid
         LEFT JOIN memory_items m ON m.document_id=c.document_id
         WHERE chunk_fts MATCH ?1
           AND (chunk_fts.source_kind<>'source_claim' OR EXISTS(
                SELECT 1 FROM source_valid_claims sc WHERE sc.document_id=c.document_id))
           AND (chunk_fts.source_kind<>'topic_summary' OR EXISTS(
                SELECT 1 FROM topic_valid_documents tv WHERE tv.document_id=c.document_id))
           AND (?2 IS NULL OR chunk_fts.scope=?2)
           AND (?3 IS NULL OR chunk_fts.source_kind=?3)
           AND (?6=0 OR chunk_fts.source_kind<>'topic_summary')
           AND (m.document_id IS NULL OR (m.superseded_by IS NULL AND (m.valid_until_ms IS NULL OR m.valid_until_ms>?4)))
         ORDER BY lexical_score,chunk_fts.rowid LIMIT ?5";
    let reordered_lexical_sql = lexical_sql
        .replace("           AND (?2 IS NULL OR chunk_fts.scope=?2)\n", "")
        .replace("           AND (?3 IS NULL OR chunk_fts.source_kind=?3)\n", "")
        .replace("           AND (?6=0 OR chunk_fts.source_kind<>'topic_summary')\n", "")
        .replace("WHERE chunk_fts MATCH ?1", "WHERE chunk_fts MATCH ?1\n           AND (?2 IS NULL OR chunk_fts.scope=?2)\n           AND (?3 IS NULL OR chunk_fts.source_kind=?3)\n           AND (?6=0 OR chunk_fts.source_kind<>'topic_summary')");
    for (name, query) in [
        (
            "lexical_cross_source",
            "Archive OR review OR withdrawal OR restore",
        ),
        ("lexical_raw", "Indigo OR sensor OR calibration"),
        (
            "lexical_empty",
            "Quartz OR spacecraft OR launch OR manifest",
        ),
    ] {
        measurements.push(profile(
            &connection,
            name,
            lexical_sql,
            vec![
                query.to_string().into(),
                SCOPE.to_string().into(),
                Value::Null,
                2_000_000_000_000i64.into(),
                256.into(),
                0.into(),
            ],
        )?);
    }
    for (name, query) in [
        (
            "natural_cross_source",
            "How does the Archive review workflow preserve withdrawal history and restore source support?",
        ),
        (
            "natural_empty",
            "What is the Quartz spacecraft launch manifest?",
        ),
    ] {
        let mut terms = query
            .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
            .map(|t| t.trim_matches('-'))
            .filter(|t| t.chars().count() >= 2)
            .map(str::to_ascii_lowercase)
            .collect::<Vec<_>>();
        terms.sort();
        terms.dedup();
        terms.truncate(24);
        let fts = terms
            .iter()
            .map(|t| format!("\"{t}\""))
            .collect::<Vec<_>>()
            .join(" OR ");
        for kind in [Some("memory"), Some("topic_summary"), None] {
            let parameters = vec![
                fts.clone().into(),
                SCOPE.to_string().into(),
                kind.map(str::to_string).into(),
                2_000_000_000_000i64.into(),
                256.into(),
                kind.is_none().into(),
            ];
            let original = profile(
                &connection,
                &format!("{name}_{}", kind.unwrap_or("sources")),
                lexical_sql,
                parameters.clone(),
            )?;
            let reordered = profile(
                &connection,
                &format!("{name}_{}_cheap_filters_first", kind.unwrap_or("sources")),
                &reordered_lexical_sql,
                parameters,
            )?;
            ensure!(
                original["result_sha256"] == reordered["result_sha256"],
                "Reordered query changed rows or ranking"
            );
            measurements.push(original);
            measurements.push(reordered);
        }
    }
    Ok(
        json!({"documents":document_count,"measurements":measurements,"cached_lexical_metadata":cached_lexical_metadata(&connection)?}),
    )
}

fn main() -> Result<()> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    ensure!(
        (1..=2).contains(&arguments.len()),
        "Expected synthetic seed and optional candidate SQL"
    );
    let snapshot = Path::new(&arguments[0]).canonicalize()?;
    let allowed = Path::new("target/packet-runtime-seeds").canonicalize()?;
    ensure!(
        snapshot.parent() == Some(allowed.as_path()),
        "Use a retained synthetic packet seed only"
    );
    ensure!(
        snapshot
            .file_name()
            .is_some_and(|name| name == "1000.sqlite" || name == "10000.sqlite"),
        "Unrecognised seed"
    );
    let bytes = std::fs::read(&snapshot)?;
    let hash = hex::encode(Sha256::digest(&bytes));
    let sql = arguments
        .get(1)
        .map(std::fs::read_to_string)
        .transpose()
        .context("Cannot read candidate SQL")?;
    let before = run(&snapshot, None)?;
    let after = sql
        .as_deref()
        .map(|sql| run(&snapshot, Some(sql)))
        .transpose()?;
    ensure!(
        hex::encode(Sha256::digest(std::fs::read(&snapshot)?)) == hash,
        "Retained snapshot changed"
    );
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "sqlite_version":rusqlite::version(),"seed_sha256":hash,"rounds":ROUNDS,
            "migration_sha256":sql.as_ref().map(|s| hex::encode(Sha256::digest(s))),
            "before":before,"after":after,
            "limitations":["SQL timings exclude the complete packet pipeline and model work.","Fixed-order local microbenchmarks; operation counts identify algorithmic work, not production latency."]
        }))?
    );
    Ok(())
}
