//! Authenticated reader sets are supplied by the transport, never by a model.
//! The local operator CLI is privileged and uses the same storage operations.
use crate::chunking::sha256_hex;
use crate::memory::distill_in_transaction;
use crate::model::{DistillInput, IngestDocument};
use crate::redaction::redact_text;
use crate::store::{Store, ingest_prepared, now_ms, prepare_ingest, random_nonce};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::{Value, json};

pub fn validate_reader(reader: &str) -> Result<()> {
    let (provider, id) = reader
        .split_once(':')
        .context("reader must have a transport namespace")?;
    ensure!(
        reader.len() <= 256 && !provider.is_empty() && !id.is_empty()
            && provider.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':' | b'@')),
        "invalid reader identifier"
    );
    Ok(())
}

pub(crate) fn canonical_readers(readers: &[String]) -> Result<String> {
    ensure!(readers.len() <= 1024, "reader count exceeds limit");
    for reader in readers {
        validate_reader(reader)?;
    }
    let mut ordered: Vec<_> = readers.iter().collect();
    ordered.sort_unstable();
    ordered.dedup();
    Ok(serde_json::to_string(&ordered)?)
}

/// The migration never relabels existing scopes. A collision (even with an
/// unpublished legacy source or an inactive document) allocates a fresh scope.
fn allocate_audience(
    tx: &Transaction<'_>,
    kind: &str,
    readers: &str,
    base: String,
) -> Result<String> {
    let scope = unused_scope(tx, &base)?;
    tx.execute(
        "INSERT INTO privacy_audiences(scope,kind,readers_json) VALUES(?1,?2,?3)",
        params![scope, kind, readers],
    )?;
    Ok(scope)
}

fn unused_scope(tx: &Transaction<'_>, base: &str) -> Result<String> {
    let mut scope = base.to_owned();
    loop {
        let occupied: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM documents WHERE scope=?1)
                 OR EXISTS(SELECT 1 FROM sources WHERE scope=?1)
                 OR EXISTS(SELECT 1 FROM topics WHERE scope=?1)
                 OR EXISTS(SELECT 1 FROM learning_runs WHERE scope=?1)
                 OR EXISTS(SELECT 1 FROM privacy_audiences WHERE scope=?1)",
            [&scope],
            |row| row.get(0),
        )?;
        if !occupied {
            break;
        }
        scope = format!("{base}:{}", random_nonce(tx)?);
    }
    Ok(scope)
}

pub(crate) fn initialise_public_audience(tx: &Transaction<'_>) -> Result<()> {
    allocate_audience(tx, "public", "[]", "public".into())?;
    Ok(())
}

fn private_audience(tx: &Transaction<'_>, readers: &str) -> Result<String> {
    ensure!(
        readers != "[]",
        "private memory requires a known nonempty audience"
    );
    if let Some(scope) = tx
        .query_row(
            "SELECT scope FROM privacy_audiences WHERE kind='private' AND readers_json=?1",
            [readers],
            |row| row.get(0),
        )
        .optional()?
    {
        return Ok(scope);
    }
    allocate_audience(
        tx,
        "private",
        readers,
        format!("private:{}", sha256_hex(readers)),
    )
}

impl Store {
    /// The delivery guard may allow new readers only when Moon injected public
    /// material exclusively. Check the bounded packet IDs against published records.
    pub fn packet_is_public(&self, packet: &crate::ContextPacket) -> Result<bool> {
        if !packet.references.is_empty()
            || packet
                .memories
                .iter()
                .any(|m| m.owner_attested_legacy || !m.citations.is_empty())
        {
            return Ok(false);
        }
        if packet.memories.is_empty() {
            return Ok(true);
        }
        let ids: Vec<_> = packet.memories.iter().map(|m| m.document_id).collect();
        Ok(self.connection.query_row(
            "SELECT NOT EXISTS (SELECT 1 FROM json_each(?1) i WHERE NOT EXISTS
               (SELECT 1 FROM memory_publications p JOIN documents d ON d.id=p.document_id
                WHERE p.document_id=CAST(i.value AS INTEGER) AND d.active=1))",
            [serde_json::to_string(&ids)?],
            |r| r.get(0),
        )?)
    }

    pub fn public_memory_scope(&self) -> Result<String> {
        Ok(self.connection.query_row(
            "SELECT scope FROM privacy_audiences WHERE kind='public'",
            [],
            |row| row.get(0),
        )?)
    }

    pub fn register_private_audience(&mut self, readers: &[String]) -> Result<String> {
        let readers = canonical_readers(readers)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let scope = private_audience(&tx, &readers)?;
        tx.commit()?;
        Ok(scope)
    }

    /// Resolve the write namespace from the immutable admission. On an exact
    /// retry preserve the original evidence scope, including pre-19 records.
    pub fn admitted_memory_scope(&mut self, key: &str, session: &str) -> Result<String> {
        let admission = self.turn_admission(key)?;
        let speaker = admission
            .sender
            .context("admission lacks a verified speaker")?;
        let owners = canonical_readers(
            &admission
                .owners
                .context("admission lacks verified owners")?,
        )?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing: Option<String> = tx.query_row(
            "SELECT d.scope FROM evidence_admissions b JOIN evidence_sessions e ON e.id=b.evidence_id
             JOIN documents d ON d.id=e.document_id WHERE b.admission_key=?1 AND e.session_id=?2",
            params![key,session], |row| row.get(0),
        ).optional()?;
        let audience = private_audience(&tx, &owners)?;
        let scope = match existing {
            Some(scope) => scope,
            None => {
                if let Some(scope) = tx.query_row(
                    "SELECT scope FROM speaker_memory_scopes WHERE audience_scope=?1 AND sender=?2",
                    params![audience,speaker], |row| row.get::<_,String>(0),
                ).optional()? { scope } else {
                    let digest = sha256_hex(&serde_json::to_string(&(&audience, &speaker))?);
                    let scope = unused_scope(&tx, &format!("private:speaker:{digest}"))?;
                    tx.execute("INSERT INTO speaker_memory_scopes(scope,audience_scope,sender) VALUES(?1,?2,?3)",
                        params![scope,audience,speaker])?;
                    scope
                }
            }
        };
        tx.commit()?;
        Ok(scope)
    }

    /// A room may recall a memory only if every current reader was in the
    /// original audience. An empty/unknown room never passes by vacuous truth.
    pub fn readable_memory_scopes(&self, readers: &[String]) -> Result<Vec<String>> {
        let readers = canonical_readers(readers)?;
        let mut stmt = self.connection.prepare(
            "SELECT scope FROM memory_audiences a WHERE a.kind='public'
               OR (?1<>'[]' AND NOT EXISTS (
                   SELECT value FROM json_each(?1) EXCEPT SELECT value FROM json_each(a.readers_json)
               )) ORDER BY a.kind='public' DESC, scope"
        )?;
        Ok(stmt
            .query_map([readers], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// An operator attests ownership of the original global collection, not authorship
    /// of its claims. This leaves canonical heads, expiry and source evidence intact.
    pub fn assign_legacy_collection(&mut self, owner: &str) -> Result<Value> {
        validate_reader(owner)?;
        let readers = serde_json::to_string(&[owner])?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing: Option<String> = tx
            .query_row(
                "SELECT owner FROM legacy_collection_owner WHERE singleton=1",
                [],
                |r| r.get(0),
            )
            .optional()?;
        match existing {
            Some(previous) => ensure!(
                previous == owner,
                "legacy collection already has another owner"
            ),
            None => {
                let scope = private_audience(&tx, &readers)?;
                tx.execute(
                    "INSERT INTO legacy_collection_owner(singleton,owner,scope,assigned_at_ms) VALUES(1,?1,?2,?3)",
                    params![owner,scope,now_ms()],
                )?;
            }
        }
        let (assigned, recallable): (i64, i64) = tx.query_row(
            "SELECT count(*), count(CASE WHEN d.active=1 AND m.superseded_by IS NULL
                  AND (m.valid_until_ms IS NULL OR m.valid_until_ms>?1) THEN 1 END)
             FROM owned_legacy_memories o JOIN documents d ON d.id=o.document_id
             JOIN memory_items m ON m.document_id=o.document_id",
            [now_ms()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        tx.commit()?;
        Ok(json!({"owner":owner,"assigned":assigned,"recallable":recallable}))
    }

    pub fn legacy_memory_inventory(&self) -> Result<Value> {
        Ok(self.connection.query_row(
            "SELECT count(*),count(m.document_id),count(e.document_id),count(r.legacy_document_id)
             FROM privacy_legacy_documents l
             LEFT JOIN memory_items m ON m.document_id=l.document_id
             LEFT JOIN evidence_sessions e ON e.document_id=l.document_id
             LEFT JOIN privacy_recoveries r ON r.legacy_document_id=l.document_id", [], |row| {
                Ok(json!({"legacy_documents":row.get::<_,i64>(0)?, "legacy_memories":row.get::<_,i64>(1)?,
                          "legacy_evidence":row.get::<_,i64>(2)?, "privately_recovered":row.get::<_,i64>(3)?}))
             })?)
    }

    /// A content-free local review queue; the caller explicitly selects a row
    /// before its attributable quote or session metadata is released.
    pub fn legacy_memory_candidates(&self, after: i64, limit: usize) -> Result<Vec<Value>> {
        ensure!(
            after >= 0 && (1..=100).contains(&limit),
            "invalid legacy review page"
        );
        let mut stmt = self.connection.prepare(
            "SELECT l.document_id, r.document_id IS NOT NULL,
                    (SELECT count(*) FROM memory_citations c WHERE c.memory_document_id=l.document_id)
             FROM privacy_legacy_documents l JOIN memory_items m ON m.document_id=l.document_id
             LEFT JOIN privacy_recoveries r ON r.legacy_document_id=l.document_id
             WHERE l.document_id>?1 ORDER BY l.document_id LIMIT ?2"
        )?;
        Ok(stmt.query_map(params![after,limit as i64], |row| Ok(json!({
            "document_id":row.get::<_,i64>(0)?,"recovered":row.get::<_,bool>(1)?,"citation_count":row.get::<_,i64>(2)?
        })))?.collect::<rusqlite::Result<_>>()?)
    }

    /// This result contains private user wording. Expose only by explicit
    /// local operator request; never return a mixed transcript or assistant text.
    pub fn legacy_memory_review(&self, id: i64) -> Result<Value> {
        Ok(serde_json::to_value(review_legacy_citation(
            &self.connection,
            id,
        )?)?)
    }

    /// Local operator review: the operator attests the historical audience
    /// independently. Only a single exact, cited user passage can be recovered.
    /// Copying the whole turn would copy any secretly injected assistant facts.
    pub fn recover_legacy_memory(
        &mut self,
        legacy_document_id: i64,
        readers: &[String],
        quote: &str,
    ) -> Result<i64> {
        ensure!(
            !quote.trim().is_empty()
                && quote == quote.trim()
                && quote.len() <= 8192
                && redact_text(quote).count == 0,
            "invalid or sensitive recovery wording"
        );
        let readers = canonical_readers(readers)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let scope = private_audience(&tx, &readers)?;
        let quote_hash = sha256_hex(quote);
        if let Some((id, previous_scope, previous_hash)) = tx.query_row(
            "SELECT document_id,scope,quote_hash FROM privacy_recoveries WHERE legacy_document_id=?1",
            [legacy_document_id], |r| Ok((r.get(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))
            .optional()? {
            ensure!(scope==previous_scope && quote_hash==previous_hash, "legacy record already recovered differently");
            return Ok(id);
        }

        let source = review_legacy_citation(&tx, legacy_document_id)?;
        ensure!(
            source.quote == quote,
            "wording differs from reviewed user citation"
        );

        let session_id = format!("recovery:{}", random_nonce(&tx)?);
        let evidence = ingest_prepared(
            &tx,
            prepare_ingest(IngestDocument {
                source_uri: format!("evidence://session/{}", &sha256_hex(&session_id)[..24]),
                source_kind: "evidence".into(),
                scope: scope.clone(),
                title: None,
                content: quote.into(),
                modified_at_ms: source.completed_at_ms,
                metadata_json: "{}".into(),
            })?,
        )?;
        tx.execute("INSERT INTO evidence_sessions(session_id,document_id,completed_at_ms,recorded_at_ms) VALUES(?1,?2,?3,?4)",
            params![session_id,evidence.document_id,source.completed_at_ms,now_ms()])?;
        let outcome = distill_in_transaction(
            &tx,
            DistillInput {
                canonical_key: format!("recovered:{legacy_document_id}"),
                memory_kind: source.memory_kind,
                scope: scope.clone(),
                title: None,
                content: quote.into(),
                evidence_quote: quote.into(),
                evidence_session_id: session_id,
                importance: 0.5,
                confidence: 1.0,
                pinned: false,
                supersedes: None,
                valid_until_ms: source.valid_until_ms,
            },
        )?;
        tx.execute("INSERT INTO privacy_recoveries(legacy_document_id,document_id,scope,quote_hash) VALUES(?1,?2,?3,?4)",
            params![legacy_document_id,outcome.document_id,scope,quote_hash])?;
        tx.commit()?;
        Ok(outcome.document_id)
    }
}

#[derive(serde::Serialize)]
struct LegacyCitation {
    document_id: i64,
    quote: String,
    memory_kind: String,
    completed_at_ms: i64,
    valid_until_ms: Option<i64>,
    session_key: String,
    admission_entry_id: String,
    terminal_entry_id: String,
}

fn review_legacy_citation(db: &Connection, id: i64) -> Result<LegacyCitation> {
    let (body, start, end, source) = db.query_row(
        "SELECT ed.body,c.start_byte,c.end_byte,c.quote,e.completed_at_ms,mi.valid_until_ms,mi.memory_kind,
                json_extract(e.metadata_json,'$.session_key'),json_extract(e.metadata_json,'$.admission_entry_id'),
                json_extract(e.metadata_json,'$.terminal_entry_id')
         FROM privacy_legacy_documents l JOIN documents m ON m.id=l.document_id AND m.active=1
         JOIN memory_items mi ON mi.document_id=m.id AND mi.superseded_by IS NULL
         JOIN memory_citations c ON c.memory_document_id=m.id
         JOIN evidence_sessions e ON e.id=c.evidence_session_id
         JOIN documents ed ON ed.id=e.document_id AND ed.id=c.evidence_document_id
         JOIN privacy_legacy_documents le ON le.document_id=ed.id
         WHERE l.document_id=?1 AND json_extract(e.metadata_json,'$.source')='openclaw'
           AND length(json_extract(e.metadata_json,'$.session_key'))>0
           AND length(json_extract(e.metadata_json,'$.admission_entry_id'))>0
           AND length(json_extract(e.metadata_json,'$.terminal_entry_id'))>0
           AND (mi.valid_until_ms IS NULL OR mi.valid_until_ms>?2)
           AND (SELECT count(*) FROM memory_citations cc WHERE cc.memory_document_id=m.id)=1",
        params![id,now_ms()], |r| Ok((r.get::<_,String>(0)?, r.get::<_,i64>(1)?, r.get::<_,i64>(2)?,
             LegacyCitation{document_id:id,quote:r.get(3)?,completed_at_ms:r.get(4)?,valid_until_ms:r.get(5)?,
                 memory_kind:r.get(6)?,session_key:r.get(7)?,admission_entry_id:r.get(8)?,terminal_entry_id:r.get(9)?})))
        .optional()?.context("legacy record lacks one attributable original user citation")?;
    let start = usize::try_from(start).context("invalid citation start")?;
    let end = usize::try_from(end).context("invalid citation end")?;
    let mut boundaries = body.match_indices("\n\nAssistant:\n");
    let user_end = boundaries
        .next()
        .context("legacy turn lacks separate roles")?
        .0;
    ensure!(
        boundaries.next().is_none()
            && body.starts_with("User:\n")
            && start >= "User:\n".len()
            && end <= user_end
            && body.get(start..end) == Some(source.quote.as_str()),
        "citation is outside the unambiguous user passage"
    );
    ensure!(
        !source.quote.trim().is_empty()
            && source.quote == source.quote.trim()
            && source.quote.len() <= 8192
            && redact_text(&source.quote).count == 0,
        "invalid or sensitive recovery wording"
    );
    Ok(source)
}
