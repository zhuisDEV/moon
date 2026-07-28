use crate::chunking::sha256_hex;
use crate::embedding::EmbeddingProvider;
use crate::model::{
    ContextCitation, ContextMemory, ContextPacket, ContextReference, ContextRequest, DistillAction,
    DistillInput, DistillOutcome, EvidenceInput, EvidenceOutcome, IngestDocument, SearchHit,
    SearchRequest,
};
use crate::redaction::{redact_json, redact_text};
use crate::store::{Store, ingest_prepared, now_ms, prepare_ingest, random_nonce};
use anyhow::Result;
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use std::collections::{BTreeMap, BTreeSet};

const MIN_CONTEXT_CHARS: usize = 512;
const MAX_CONTEXT_CHARS: usize = 32_000;
const MAX_CITATION_QUOTE_CHARS: usize = 500;
const MAX_EVIDENCE_QUOTE_BYTES: usize = 8 * 1024;

impl Store {
    pub fn record_evidence(&mut self, input: EvidenceInput) -> Result<EvidenceOutcome> {
        validate_session_id(&input.session_id)?;
        validate_identifier("scope", &input.scope, 2_048)?;
        if input.content.trim().is_empty() {
            anyhow::bail!("evidence content must not be empty");
        }
        if input.completed_at_ms <= 0 {
            anyhow::bail!("completed_at_ms must be a positive Unix timestamp in milliseconds");
        }

        let redacted_content = redact_text(&input.content);
        let redacted_metadata = redact_json(&input.metadata_json)?;
        let redacted_title = input.title.as_deref().map(redact_text);
        let redactions = redacted_content.count
            + redacted_metadata.count
            + redacted_title.as_ref().map_or(0, |redacted| redacted.count);
        let sanitized_title = redacted_title.map(|redacted| redacted.value);
        let metadata_value: serde_json::Value = serde_json::from_str(&redacted_metadata.value)?;
        let document_metadata = serde_json::json!({
            "session_id": input.session_id,
            "completed_at_ms": input.completed_at_ms,
            "metadata": metadata_value,
        })
        .to_string();
        let content_hash = sha256_hex(&redacted_content.value);

        let session_hash = sha256_hex(&input.session_id);
        let prepared = prepare_ingest(IngestDocument {
            source_uri: format!("evidence://session/{}", &session_hash[..24]),
            source_kind: "evidence".to_string(),
            scope: input.scope.clone(),
            title: sanitized_title.clone(),
            content: redacted_content.value,
            modified_at_ms: input.completed_at_ms,
            metadata_json: document_metadata,
        })?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = transaction
            .query_row(
                "SELECT e.document_id, d.content_hash, d.scope, d.title, e.completed_at_ms,
                        e.metadata_json, e.redactions,
                        (SELECT count(*) FROM chunks c WHERE c.document_id = d.id)
                 FROM evidence_sessions e
                 JOIN documents d ON d.id = e.document_id
                 WHERE e.session_id = ?1",
                [&input.session_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, i64>(7)?,
                    ))
                },
            )
            .optional()?;

        if let Some((
            document_id,
            existing_hash,
            scope,
            existing_title,
            completed_at_ms,
            metadata_json,
            existing_redactions,
            chunks,
        )) = existing
        {
            if existing_hash != content_hash
                || scope != input.scope
                || existing_title != sanitized_title
                || completed_at_ms != input.completed_at_ms
                || metadata_json != redacted_metadata.value
            {
                anyhow::bail!(
                    "evidence session `{}` is immutable and already has different content or metadata",
                    input.session_id
                );
            }
            let outcome = EvidenceOutcome {
                document_id,
                session_id: input.session_id,
                chunks: chunks as usize,
                changed: false,
                redactions: existing_redactions as usize,
            };
            transaction.commit()?;
            return Ok(outcome);
        }

        let outcome = ingest_prepared(&transaction, prepared)?;
        transaction.execute(
            "INSERT INTO evidence_sessions(
                 session_id, document_id, completed_at_ms, recorded_at_ms, redactions, metadata_json
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                input.session_id,
                outcome.document_id,
                input.completed_at_ms,
                now_ms(),
                redactions as i64,
                redacted_metadata.value,
            ],
        )?;
        transaction.commit()?;

        Ok(EvidenceOutcome {
            document_id: outcome.document_id,
            session_id: input.session_id,
            chunks: outcome.chunks,
            changed: true,
            redactions,
        })
    }

    pub fn distill_memory(&mut self, input: DistillInput) -> Result<DistillOutcome> {
        validate_canonical_key(&input.canonical_key)?;
        validate_identifier("memory_kind", &input.memory_kind, 128)?;
        validate_identifier("scope", &input.scope, 2_048)?;
        validate_unit_interval("importance", input.importance)?;
        validate_unit_interval("confidence", input.confidence)?;
        if input.content.trim().is_empty() {
            anyhow::bail!("memory content must not be empty");
        }
        if input.evidence_quote.trim().chars().count() < 8 {
            anyhow::bail!("evidence_quote must contain at least 8 characters");
        }
        if input.evidence_quote.len() > MAX_EVIDENCE_QUOTE_BYTES {
            anyhow::bail!(
                "evidence_quote exceeds the maximum size of {MAX_EVIDENCE_QUOTE_BYTES} bytes"
            );
        }

        let redacted_content = redact_text(&input.content);
        let redacted_quote = redact_text(&input.evidence_quote);
        let redacted_title = input.title.as_deref().map(redact_text);
        let redactions = redacted_content.count
            + redacted_quote.count
            + redacted_title.as_ref().map_or(0, |redacted| redacted.count);
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let evidence = load_evidence(&transaction, &input.evidence_session_id)?;
        let citation = locate_citation(&evidence.body, redacted_quote.value.trim())?;
        let content_hash = sha256_hex(&redacted_content.value);

        let head = transaction
            .query_row(
                "SELECT h.document_id, d.content_hash
                 FROM memory_heads h
                 JOIN documents d ON d.id = h.document_id
                 JOIN memory_items m ON m.document_id = h.document_id
                 WHERE h.canonical_key = ?1
                   AND d.active = 1
                   AND m.superseded_by IS NULL",
                [&input.canonical_key],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;

        if let Some((document_id, existing_hash)) = head.as_ref()
            && existing_hash == &content_hash
        {
            if input.supersedes.is_some() {
                anyhow::bail!(
                    "memory `{}` already has this content; omit supersedes to confirm it",
                    input.canonical_key
                );
            }
            transaction.execute(
                "UPDATE memory_items
                 SET importance = max(importance, ?2),
                     confidence = max(confidence, ?3),
                     pinned = max(pinned, ?4),
                     last_confirmed_at_ms = ?5
                 WHERE document_id = ?1",
                params![
                    document_id,
                    input.importance,
                    input.confidence,
                    input.pinned,
                    now_ms(),
                ],
            )?;
            insert_citation(
                &transaction,
                *document_id,
                &evidence,
                &citation,
                redacted_quote.value.trim(),
            )?;
            let evidence_count = citation_count(&transaction, *document_id)?;
            transaction.commit()?;
            return Ok(DistillOutcome {
                document_id: *document_id,
                canonical_key: input.canonical_key,
                action: DistillAction::Confirmed,
                superseded_document_id: None,
                evidence_count,
                redactions,
            });
        }

        let superseded_document_id = match head {
            Some((document_id, _)) => {
                if input.supersedes != Some(document_id) {
                    anyhow::bail!(
                        "memory `{}` already has different active content in document {document_id}; pass --supersedes {document_id} after review",
                        input.canonical_key
                    );
                }
                Some(document_id)
            }
            None => {
                if input.supersedes.is_some() {
                    anyhow::bail!(
                        "memory `{}` has no active head to supersede",
                        input.canonical_key
                    );
                }
                None
            }
        };

        let key_hash = sha256_hex(&input.canonical_key);
        let nonce = random_nonce(&transaction)?;
        let prepared = prepare_ingest(IngestDocument {
            source_uri: format!("memory://canonical/{}/revision/{}", &key_hash[..20], nonce),
            source_kind: "memory".to_string(),
            scope: input.scope,
            title: redacted_title.map(|redacted| redacted.value),
            content: redacted_content.value,
            modified_at_ms: now_ms(),
            metadata_json: serde_json::json!({
                "canonical_key": input.canonical_key,
                "evidence_session_id": input.evidence_session_id,
            })
            .to_string(),
        })?;
        let outcome = ingest_prepared(&transaction, prepared)?;
        transaction.execute(
            "INSERT INTO memory_items(
                 document_id, memory_kind, importance, confidence, valid_from_ms, pinned,
                 canonical_key, last_confirmed_at_ms
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, NULL, ?5)",
            params![
                outcome.document_id,
                input.memory_kind,
                input.importance,
                input.confidence,
                now_ms(),
                input.pinned,
            ],
        )?;
        if let Some(previous_document_id) = superseded_document_id {
            let updated = transaction.execute(
                "UPDATE memory_items
                 SET superseded_by = ?2, valid_until_ms = ?3
                 WHERE document_id = ?1 AND superseded_by IS NULL",
                params![previous_document_id, outcome.document_id, now_ms()],
            )?;
            if updated != 1 {
                anyhow::bail!(
                    "active memory changed during supersession; retry after reviewing the current head"
                );
            }
            transaction.execute(
                "DELETE FROM chunk_fts
                 WHERE rowid IN (
                     SELECT id FROM chunks WHERE document_id = ?1
                 )",
                [previous_document_id],
            )?;
            transaction.execute(
                "DELETE FROM chunk_vectors
                 WHERE rowid IN (
                     SELECT id FROM chunks WHERE document_id = ?1
                 )",
                [previous_document_id],
            )?;
            transaction.execute(
                "DELETE FROM embedding_queue
                 WHERE chunk_id IN (
                     SELECT id FROM chunks WHERE document_id = ?1
                 )",
                [previous_document_id],
            )?;
            transaction.execute(
                "UPDATE chunks
                 SET embedding_model = NULL, embedded_at_ms = NULL
                 WHERE document_id = ?1",
                [previous_document_id],
            )?;
        }
        transaction.execute(
            "UPDATE memory_items SET canonical_key = ?2 WHERE document_id = ?1",
            params![outcome.document_id, input.canonical_key],
        )?;
        transaction.execute(
            "INSERT INTO memory_heads(canonical_key, document_id, updated_at_ms)
             VALUES(?1, ?2, ?3)
             ON CONFLICT(canonical_key) DO UPDATE SET
                 document_id = excluded.document_id,
                 updated_at_ms = excluded.updated_at_ms",
            params![input.canonical_key, outcome.document_id, now_ms()],
        )?;
        insert_citation(
            &transaction,
            outcome.document_id,
            &evidence,
            &citation,
            redacted_quote.value.trim(),
        )?;
        let evidence_count = citation_count(&transaction, outcome.document_id)?;
        transaction.commit()?;

        Ok(DistillOutcome {
            document_id: outcome.document_id,
            canonical_key: input.canonical_key,
            action: if superseded_document_id.is_some() {
                DistillAction::Superseded
            } else {
                DistillAction::Created
            },
            superseded_document_id,
            evidence_count,
            redactions,
        })
    }

    pub fn assemble_context(
        &self,
        request: &ContextRequest,
        provider: Option<&dyn EmbeddingProvider>,
    ) -> Result<ContextPacket> {
        if request.query.trim().is_empty() {
            anyhow::bail!("context query must not be empty");
        }
        if !(MIN_CONTEXT_CHARS..=MAX_CONTEXT_CHARS).contains(&request.max_chars) {
            anyhow::bail!("max_chars must be between {MIN_CONTEXT_CHARS} and {MAX_CONTEXT_CHARS}");
        }
        let limit = request.limit.clamp(1, 64);
        let evidence_limit = request.evidence_per_memory.clamp(0, 8);
        let hits = self
            .search(
                &SearchRequest {
                    query: request.query.clone(),
                    mode: request.mode,
                    limit: limit.saturating_mul(4).min(100),
                    scope: request.scope.clone(),
                    source_kind: Some("memory".to_string()),
                },
                provider,
            )?
            .into_iter()
            .filter(|hit| is_relevant_search_hit(&request.query, hit))
            .collect::<Vec<_>>();

        let relevant_ids = hits
            .iter()
            .map(|hit| hit.document_id)
            .collect::<BTreeSet<_>>();
        let mut candidates = BTreeMap::<i64, f64>::new();
        for hit in hits {
            candidates
                .entry(hit.document_id)
                .and_modify(|score| *score = score.max(hit.score))
                .or_insert(hit.score);
        }
        for document_id in self.pinned_summary_ids(request.scope.as_deref(), limit)? {
            candidates
                .entry(document_id)
                .and_modify(|score| *score += 1.0)
                .or_insert(1.0);
        }

        let mut memories = candidates
            .into_iter()
            .map(|(document_id, score)| {
                self.load_context_memory(document_id, score, evidence_limit)
            })
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        memories.sort_by(|left, right| {
            right
                .pinned
                .cmp(&left.pinned)
                .then_with(|| {
                    let left_summary = left.memory_kind == "summary";
                    let right_summary = right.memory_kind == "summary";
                    right_summary.cmp(&left_summary)
                })
                .then_with(|| right.relevance_score.total_cmp(&left.relevance_score))
                .then_with(|| right.importance.total_cmp(&left.importance))
                .then_with(|| left.document_id.cmp(&right.document_id))
        });

        let candidate_count = memories.len();
        let relevant_available = memories
            .iter()
            .any(|memory| relevant_ids.contains(&memory.document_id));
        let pinned_quota = if relevant_available {
            limit.div_ceil(3).min(limit.saturating_sub(1))
        } else {
            limit
        };
        let mut selected_ids = BTreeSet::new();
        let mut selected = Vec::with_capacity(limit);
        for memory in memories.iter().filter(|memory| memory.pinned) {
            if selected.len() >= pinned_quota {
                break;
            }
            selected_ids.insert(memory.document_id);
            selected.push(memory.clone());
        }
        for memory in memories
            .iter()
            .filter(|memory| relevant_ids.contains(&memory.document_id))
        {
            if selected.len() >= limit {
                break;
            }
            if selected_ids.insert(memory.document_id) {
                selected.push(memory.clone());
            }
        }
        for memory in &memories {
            if selected.len() >= limit {
                break;
            }
            if selected_ids.insert(memory.document_id) {
                selected.push(memory.clone());
            }
        }
        let mut packet = ContextPacket {
            query: truncate_chars(request.query.trim(), 160),
            scope: request
                .scope
                .as_deref()
                .map(|scope| truncate_chars(scope, 80)),
            max_chars: request.max_chars,
            used_chars: 0,
            truncated: false,
            memories: Vec::new(),
            references: Vec::new(),
        };

        for mut memory in selected {
            if push_if_fits(&mut packet, memory.clone()) {
                continue;
            }
            packet.truncated = true;
            memory.citations.truncate(1);
            if let Some(citation) = memory.citations.first_mut() {
                truncate_citation_quote(citation, 240);
            }
            if push_if_fits(&mut packet, memory.clone()) {
                continue;
            }

            let mut shell = memory.clone();
            shell.content.clear();
            let base_length = packet.render_markdown().chars().count();
            packet.memories.push(shell);
            let shell_length = packet.render_markdown().chars().count();
            packet.memories.pop();
            let overhead = shell_length.saturating_sub(base_length);
            let available = request
                .max_chars
                .saturating_sub(base_length)
                .saturating_sub(overhead);
            if available >= 80 {
                memory.content = truncate_chars(&memory.content, available);
                let _ = push_if_fits(&mut packet, memory);
            }
        }
        if candidate_count > limit {
            packet.truncated = true;
        }
        let remaining_slots = limit.saturating_sub(packet.memories.len());
        if remaining_slots > 0 {
            let reference_hits = self
                .search(
                    &SearchRequest {
                        query: request.query.clone(),
                        mode: request.mode,
                        limit: remaining_slots.saturating_mul(8).min(100),
                        scope: request.scope.clone(),
                        source_kind: None,
                    },
                    provider,
                )?
                .into_iter()
                .filter(|hit| is_relevant_search_hit(&request.query, hit))
                .collect::<Vec<_>>();
            let mut seen_documents = BTreeSet::new();
            let references = reference_hits
                .into_iter()
                .filter(|hit| !matches!(hit.source_kind.as_str(), "memory" | "evidence"))
                .filter(|hit| seen_documents.insert(hit.document_id))
                .map(|hit| self.load_context_reference(hit))
                .collect::<Result<Vec<_>>>()?;
            if references.len() > remaining_slots {
                packet.truncated = true;
            }
            for mut reference in references.into_iter().take(remaining_slots) {
                if push_reference_if_fits(&mut packet, reference.clone()) {
                    continue;
                }
                packet.truncated = true;
                let base_length = packet.render_markdown().chars().count();
                let full_content = reference.content.clone();
                reference.content.clear();
                packet.references.push(reference.clone());
                let shell_length = packet.render_markdown().chars().count();
                packet.references.pop();
                let overhead = shell_length.saturating_sub(base_length);
                let available = request
                    .max_chars
                    .saturating_sub(base_length)
                    .saturating_sub(overhead);
                if available >= 80 {
                    reference.content = full_content;
                    truncate_reference_content(&mut reference, available);
                    let _ = push_reference_if_fits(&mut packet, reference);
                }
            }
        }
        packet.used_chars = packet.render_markdown().chars().count();
        Ok(packet)
    }

    fn load_context_reference(&self, hit: SearchHit) -> Result<ContextReference> {
        let (body, approximate_start) = self.connection.query_row(
            "SELECT d.body, c.start_byte
             FROM chunks c
             JOIN documents d ON d.id = c.document_id
             WHERE c.id = ?1 AND c.document_id = ?2 AND d.active = 1",
            params![hit.chunk_id, hit.document_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize)),
        )?;
        let (start_byte, end_byte) = exact_reference_range(&body, &hit.content, approximate_start)?;
        Ok(ContextReference {
            document_id: hit.document_id,
            chunk_id: hit.chunk_id,
            source_uri: hit.source_uri,
            source_kind: hit.source_kind,
            scope: hit.scope,
            title: hit.title,
            content: hit.content,
            relevance_score: hit.score,
            start_byte,
            end_byte,
        })
    }

    fn pinned_summary_ids(&self, scope: Option<&str>, limit: usize) -> Result<Vec<i64>> {
        let mut statement = self.connection.prepare(
            "SELECT m.document_id
             FROM memory_items m
             JOIN documents d ON d.id = m.document_id
             WHERE d.active = 1
               AND m.pinned = 1
               AND m.memory_kind = 'summary'
               AND m.superseded_by IS NULL
               AND (m.valid_until_ms IS NULL OR m.valid_until_ms > ?2)
               AND (
                   (?1 IS NULL AND d.scope = 'global')
                   OR (?1 IS NOT NULL AND (d.scope = ?1 OR d.scope = 'global'))
               )
             ORDER BY (m.memory_kind = 'summary') DESC,
                      m.importance DESC, m.last_confirmed_at_ms DESC, m.document_id
             LIMIT ?3",
        )?;
        Ok(statement
            .query_map(params![scope, now_ms(), limit as i64], |row| {
                row.get::<_, i64>(0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn load_context_memory(
        &self,
        document_id: i64,
        relevance_score: f64,
        evidence_limit: usize,
    ) -> Result<Option<ContextMemory>> {
        let memory = self
            .connection
            .query_row(
                "SELECT d.id, m.canonical_key, m.memory_kind, d.scope, d.title, d.body,
                        m.importance, m.confidence, m.pinned
                 FROM memory_items m
                 JOIN documents d ON d.id = m.document_id
                 WHERE d.id = ?1
                   AND d.active = 1
                   AND m.superseded_by IS NULL
                   AND (m.valid_until_ms IS NULL OR m.valid_until_ms > ?2)",
                params![document_id, now_ms()],
                |row| {
                    Ok(ContextMemory {
                        document_id: row.get(0)?,
                        canonical_key: row.get(1)?,
                        memory_kind: row.get(2)?,
                        scope: row.get(3)?,
                        title: row.get(4)?,
                        content: row.get(5)?,
                        importance: row.get(6)?,
                        confidence: row.get(7)?,
                        pinned: row.get(8)?,
                        relevance_score,
                        citations: Vec::new(),
                    })
                },
            )
            .optional()?;
        let Some(mut memory) = memory else {
            return Ok(None);
        };
        memory.citations = self.load_context_citations(document_id, evidence_limit)?;
        Ok(Some(memory))
    }

    fn load_context_citations(
        &self,
        document_id: i64,
        limit: usize,
    ) -> Result<Vec<ContextCitation>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut statement = self.connection.prepare(
            "SELECT e.session_id, d.source_uri, c.start_byte, c.end_byte,
                    c.start_line, c.end_line, c.quote
             FROM memory_citations c
             JOIN evidence_sessions e ON e.id = c.evidence_session_id
             JOIN documents d ON d.id = c.evidence_document_id
             WHERE c.memory_document_id = ?1
             ORDER BY c.created_at_ms DESC, c.id DESC
             LIMIT ?2",
        )?;
        Ok(statement
            .query_map(params![document_id, limit as i64], |row| {
                Ok(ContextCitation {
                    session_id: row.get(0)?,
                    source_uri: row.get(1)?,
                    start_byte: row.get::<_, i64>(2)? as usize,
                    end_byte: row.get::<_, i64>(3)? as usize,
                    start_line: row.get::<_, i64>(4)? as usize,
                    end_line: row.get::<_, i64>(5)? as usize,
                    quote: truncate_chars(&row.get::<_, String>(6)?, MAX_CITATION_QUOTE_CHARS),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

fn is_relevant_context_hit(query: &str, content: &str) -> bool {
    fuzzy_context_score(query, content).is_some()
}

fn is_relevant_search_hit(query: &str, hit: &SearchHit) -> bool {
    if let Some(title) = hit.title.as_deref() {
        fuzzy_context_score(query, &format!("{title}\n{}", hit.content)).is_some()
    } else {
        is_relevant_context_hit(query, &hit.content)
    }
}

pub(crate) fn fuzzy_context_score(query: &str, content: &str) -> Option<f64> {
    let terms = meaningful_terms(query);
    if terms.is_empty() {
        return None;
    }
    let content_terms = normalized_terms(content);
    if content_terms.is_empty() {
        return None;
    }
    let anchors = query
        .split(|character: char| !character.is_alphanumeric())
        .filter(|term| {
            term.len() >= 3
                && (term.chars().all(|character| character.is_numeric())
                    || term.chars().next().is_some_and(char::is_uppercase)
                        && term.chars().skip(1).any(char::is_lowercase))
        })
        .map(normalize_term)
        .filter(|term| terms.contains(term))
        .collect::<Vec<_>>();
    if !anchors.is_empty()
        && !anchors
            .iter()
            .any(|anchor| content_terms.iter().any(|term| terms_match(anchor, term)))
    {
        return None;
    }
    let matched = terms
        .iter()
        .filter(|term| {
            content_terms
                .iter()
                .any(|content_term| terms_match(term, content_term))
        })
        .count();
    let accepted = if terms.len() <= 2 {
        matched >= 1
    } else {
        matched >= 2 && (matched as f64 / terms.len() as f64) >= 0.34
    };
    if accepted {
        Some(matched as f64 / terms.len() as f64)
    } else {
        None
    }
}

fn meaningful_terms(value: &str) -> Vec<String> {
    const STOP_WORDS: &[&str] = &[
        "about", "after", "again", "also", "and", "are", "can", "could", "does", "for", "from",
        "have", "hello", "help", "how", "into", "just", "like", "more", "please", "recall",
        "remember", "that", "the", "their", "then", "there", "this", "those", "use", "what",
        "when", "where", "which", "with", "would", "you", "your",
    ];
    let raw_terms = normalized_terms(value);
    if raw_terms.len() <= 3
        && raw_terms
            .first()
            .is_some_and(|term| matches!(term.as_str(), "hi" | "hello" | "hey"))
    {
        return Vec::new();
    }
    let mut terms = raw_terms
        .into_iter()
        .filter(|term| term.chars().count() >= 3)
        .filter(|term| !STOP_WORDS.contains(&term.as_str()))
        .collect::<Vec<_>>();
    terms.sort();
    terms.dedup();
    terms
}

fn normalized_terms(value: &str) -> Vec<String> {
    value
        .split(|character: char| !character.is_alphanumeric())
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(normalize_term)
        .collect()
}

fn normalize_term(value: &str) -> String {
    let mut term = value.to_lowercase();
    let length = term.chars().count();
    if length > 4 && term.ends_with("ies") {
        term.truncate(term.len() - 3);
        term.push('y');
    } else if length > 5 && term.ends_with("ing") {
        term.truncate(term.len() - 3);
        collapse_doubled_ending(&mut term);
    } else if length > 4 && term.ends_with("ed") {
        term.truncate(term.len() - 2);
        collapse_doubled_ending(&mut term);
    } else if length > 4
        && ["ses", "xes", "zes", "ches", "shes", "oes"]
            .iter()
            .any(|suffix| term.ends_with(suffix))
    {
        term.truncate(term.len() - 2);
    } else if length > 3 && term.ends_with('s') && !term.ends_with("ss") {
        term.pop();
    }
    term
}

fn collapse_doubled_ending(term: &mut String) {
    let mut characters = term.char_indices().rev();
    let Some((last_index, last)) = characters.next() else {
        return;
    };
    if characters
        .next()
        .is_some_and(|(_, previous)| previous == last)
    {
        term.truncate(last_index);
    }
}

fn terms_match(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    let length = left.chars().count().max(right.chars().count());
    let allowed = match length {
        0..=4 => 0,
        5..=7 => 1,
        _ => 2,
    };
    allowed > 0 && bounded_damerau_levenshtein(left, right, allowed).is_some()
}

fn bounded_damerau_levenshtein(left: &str, right: &str, limit: usize) -> Option<usize> {
    let left = left.chars().collect::<Vec<_>>();
    let right = right.chars().collect::<Vec<_>>();
    if left.len().abs_diff(right.len()) > limit {
        return None;
    }
    let mut previous_previous = vec![0usize; right.len() + 1];
    let mut previous = (0..=right.len()).collect::<Vec<_>>();
    for (left_index, left_character) in left.iter().enumerate() {
        let mut current = vec![left_index + 1; right.len() + 1];
        let mut row_minimum = current[0];
        for (right_index, right_character) in right.iter().enumerate() {
            let substitution =
                previous[right_index] + usize::from(left_character != right_character);
            let insertion = current[right_index] + 1;
            let deletion = previous[right_index + 1] + 1;
            current[right_index + 1] = substitution.min(insertion).min(deletion);
            if left_index > 0
                && right_index > 0
                && left_character == &right[right_index - 1]
                && left[left_index - 1] == *right_character
            {
                current[right_index + 1] =
                    current[right_index + 1].min(previous_previous[right_index - 1] + 1);
            }
            row_minimum = row_minimum.min(current[right_index + 1]);
        }
        if row_minimum > limit {
            return None;
        }
        previous_previous = previous;
        previous = current;
    }
    (previous[right.len()] <= limit).then_some(previous[right.len()])
}

fn load_evidence(transaction: &Transaction<'_>, session_id: &str) -> Result<EvidenceRecord> {
    transaction
        .query_row(
            "SELECT e.id, e.document_id, d.body
             FROM evidence_sessions e
             JOIN documents d ON d.id = e.document_id
             WHERE e.session_id = ?1 AND d.active = 1",
            [session_id],
            |row| {
                Ok(EvidenceRecord {
                    id: row.get(0)?,
                    document_id: row.get(1)?,
                    body: row.get(2)?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| anyhow::anyhow!("evidence session `{session_id}` was not recorded"))
}

#[derive(Debug)]
struct EvidenceRecord {
    id: i64,
    document_id: i64,
    body: String,
}

#[derive(Debug)]
struct CitationLocation {
    start_byte: usize,
    end_byte: usize,
    start_line: usize,
    end_line: usize,
}

fn locate_citation(body: &str, quote: &str) -> Result<CitationLocation> {
    let matches = body.match_indices(quote).collect::<Vec<_>>();
    if matches.is_empty() {
        anyhow::bail!("evidence_quote was not found exactly in the recorded evidence");
    }
    if matches.len() > 1 {
        anyhow::bail!("evidence_quote occurs more than once; provide a longer unique quote");
    }
    let start_byte = matches[0].0;
    let end_byte = start_byte + quote.len();
    let start_line = body[..start_byte]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1;
    let end_line = body[..end_byte]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1;
    Ok(CitationLocation {
        start_byte,
        end_byte,
        start_line,
        end_line,
    })
}

fn insert_citation(
    transaction: &Transaction<'_>,
    memory_document_id: i64,
    evidence: &EvidenceRecord,
    location: &CitationLocation,
    quote: &str,
) -> Result<()> {
    transaction.execute(
        "INSERT OR IGNORE INTO memory_citations(
             memory_document_id, evidence_session_id, evidence_document_id,
             start_byte, end_byte, start_line, end_line, quote, created_at_ms
         ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            memory_document_id,
            evidence.id,
            evidence.document_id,
            location.start_byte as i64,
            location.end_byte as i64,
            location.start_line as i64,
            location.end_line as i64,
            quote,
            now_ms(),
        ],
    )?;
    Ok(())
}

fn citation_count(transaction: &Transaction<'_>, memory_document_id: i64) -> Result<usize> {
    Ok(transaction.query_row(
        "SELECT count(*) FROM memory_citations WHERE memory_document_id = ?1",
        [memory_document_id],
        |row| row.get::<_, i64>(0),
    )? as usize)
}

fn push_if_fits(packet: &mut ContextPacket, memory: ContextMemory) -> bool {
    packet.memories.push(memory);
    if packet.render_markdown().chars().count() <= packet.max_chars {
        true
    } else {
        packet.memories.pop();
        false
    }
}

fn push_reference_if_fits(packet: &mut ContextPacket, reference: ContextReference) -> bool {
    packet.references.push(reference);
    if packet.render_markdown().chars().count() <= packet.max_chars {
        true
    } else {
        packet.references.pop();
        false
    }
}

fn exact_reference_range(
    body: &str,
    content: &str,
    approximate_start: usize,
) -> Result<(usize, usize)> {
    body.match_indices(content)
        .min_by_key(|(start, _)| start.abs_diff(approximate_start))
        .map(|(start, matched)| (start, start + matched.len()))
        .ok_or_else(|| anyhow::anyhow!("retrieved chunk content is not present in its document"))
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    if max_chars <= 1 {
        return "…".chars().take(max_chars).collect();
    }
    let mut output = value.chars().take(max_chars - 1).collect::<String>();
    output.push('…');
    output
}

fn truncate_reference_content(reference: &mut ContextReference, max_chars: usize) {
    truncate_exact_prefix(&mut reference.content, max_chars);
    reference.end_byte = reference.start_byte + reference.content.len();
}

fn truncate_citation_quote(citation: &mut ContextCitation, max_chars: usize) {
    truncate_exact_prefix(&mut citation.quote, max_chars);
    citation.end_byte = citation.start_byte + citation.quote.len();
    citation.end_line = citation.start_line + citation.quote.matches('\n').count();
}

fn truncate_exact_prefix(value: &mut String, max_chars: usize) {
    if let Some((byte_index, _)) = value.char_indices().nth(max_chars) {
        value.truncate(byte_index);
    }
}

fn validate_identifier(name: &str, value: &str, max_len: usize) -> Result<()> {
    if value.trim().is_empty() {
        anyhow::bail!("{name} must not be empty");
    }
    if value.len() > max_len {
        anyhow::bail!("{name} is too long");
    }
    Ok(())
}

fn validate_canonical_key(value: &str) -> Result<()> {
    validate_identifier("canonical_key", value, 256)?;
    if !value.chars().all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | ':' | '/' | '-')
    }) {
        anyhow::bail!(
            "canonical_key may contain only ASCII letters, digits, `.`, `_`, `:`, `/`, and `-`"
        );
    }
    Ok(())
}

fn validate_session_id(value: &str) -> Result<()> {
    validate_identifier("session_id", value, 256)?;
    if !value.chars().all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | ':' | '/' | '-' | '@')
    }) {
        anyhow::bail!(
            "session_id may contain only ASCII letters, digits, `.`, `_`, `:`, `/`, `-`, and `@`"
        );
    }
    Ok(())
}

fn validate_unit_interval(name: &str, value: f64) -> Result<()> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        anyhow::bail!("{name} must be between 0 and 1");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        exact_reference_range, is_relevant_context_hit, locate_citation, meaningful_terms,
        truncate_chars, truncate_citation_quote, truncate_reference_content,
    };

    #[test]
    fn relevance_filter_rejects_greetings_and_weak_relaxed_matches() {
        assert!(meaningful_terms("Hi lilac").is_empty());
        assert!(!is_relevant_context_hit(
            "Einstein Ulm 1879 exact chart",
            "A generic chart calculation guide with no subject data."
        ));
        assert!(is_relevant_context_hit(
            "Einstein Ulm 1879 exact chart",
            "Albert Einstein was born in Ulm in 1879; calculate the exact chart."
        ));
        assert!(is_relevant_context_hit(
            "astro.faith Lilacsky chart API",
            "Call the Lilacsky chart API through astro.faith."
        ));
    }

    #[test]
    fn relevance_filter_tolerates_inflection_and_bounded_typos() {
        assert!(is_relevant_context_hit(
            "search memories",
            "Moon searches one durable memory at a time."
        ));
        assert!(is_relevant_context_hit(
            "multilingaul embeding workflow",
            "The multilingual embedding workflow runs automatically."
        ));
        assert!(!is_relevant_context_hit(
            "multilingaul embeding workflow",
            "The calendar contains a lunch appointment."
        ));
    }
    use crate::{ContextCitation, ContextReference};

    #[test]
    fn citation_location_uses_bytes_and_lines() {
        let body = "first\n🌙 Moon uses SQLite.\nlast";
        let location = locate_citation(body, "🌙 Moon uses SQLite.").expect("citation");
        assert_eq!(location.start_line, 2);
        assert_eq!(location.end_line, 2);
        assert_eq!(
            &body[location.start_byte..location.end_byte],
            "🌙 Moon uses SQLite."
        );
    }

    #[test]
    fn truncation_preserves_utf8_boundaries() {
        assert_eq!(truncate_chars("Moon 🌙 memory", 7), "Moon 🌙…");
    }

    #[test]
    fn reference_range_repairs_legacy_trimmed_offsets() {
        let body = "\n  repeated text\nother\nrepeated text\n";
        assert_eq!(
            exact_reference_range(body, "repeated text", 25).expect("range"),
            (23, 36)
        );
    }

    #[test]
    fn truncated_reference_keeps_an_exact_byte_range() {
        let mut reference = ContextReference {
            document_id: 1,
            chunk_id: 2,
            source_uri: "legacy:///tmp/source.md".to_string(),
            source_kind: "legacy-memory".to_string(),
            scope: "global".to_string(),
            title: None,
            content: "Moon 🌙 memory".to_string(),
            relevance_score: 1.0,
            start_byte: 10,
            end_byte: 26,
        };
        truncate_reference_content(&mut reference, 6);
        assert_eq!(reference.content, "Moon 🌙");
        assert_eq!(
            reference.end_byte,
            reference.start_byte + reference.content.len()
        );
    }

    #[test]
    fn truncated_citation_keeps_exact_bytes_and_lines() {
        let mut citation = ContextCitation {
            session_id: "session".to_string(),
            source_uri: "evidence://session".to_string(),
            start_byte: 20,
            end_byte: 50,
            start_line: 3,
            end_line: 5,
            quote: "line one\nline two\nline three".to_string(),
        };
        truncate_citation_quote(&mut citation, 18);
        assert_eq!(citation.quote, "line one\nline two\n");
        assert_eq!(
            citation.end_byte,
            citation.start_byte + citation.quote.len()
        );
        assert_eq!(citation.end_line, 5);
    }
}
