use crate::chunking::sha256_hex;
use crate::embedding::EmbeddingProvider;
use crate::model::{
    ContextCitation, ContextMemory, ContextPacket, ContextReference, ContextRequest, DistillAction,
    DistillInput, DistillOutcome, EvidenceInput, EvidenceOutcome, IngestDocument, SearchHit,
    SearchRequest, SourceClaimCitation,
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
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let outcome = distill_in_transaction(&transaction, input)?;
        transaction.commit()?;
        Ok(outcome)
    }

    pub fn distill_batch(&mut self, inputs: Vec<DistillInput>) -> Result<Vec<DistillOutcome>> {
        if inputs.len() > 32 {
            anyhow::bail!("distillation batch may contain at most 32 proposals");
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let outcomes = inputs
            .into_iter()
            .map(|input| distill_in_transaction(&transaction, input))
            .collect::<Result<Vec<_>>>()?;
        transaction.commit()?;
        Ok(outcomes)
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

            let base_length = packet.render_markdown().chars().count();
            let mut available = 0;
            // Keep recency metadata and a useful claim even in a small packet.
            // Shorten its exact quote before dropping the whole memory.
            for quote_chars in [240, 120, 64] {
                if let Some(citation) = memory.citations.first_mut() {
                    truncate_citation_quote(citation, quote_chars);
                }
                let mut shell = memory.clone();
                shell.content.clear();
                packet.memories.push(shell);
                let shell_length = packet.render_markdown().chars().count();
                packet.memories.pop();
                let overhead = shell_length.saturating_sub(base_length);
                available = request
                    .max_chars
                    .saturating_sub(base_length)
                    .saturating_sub(overhead);
                if available >= 80 {
                    break;
                }
            }
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
            // References may answer one part of a multi-part question. Keep the
            // stricter personal-memory/correction matcher above unchanged.
            let topic_limit = (remaining_slots / 3).min(2);
            let mut topic_hits = if topic_limit > 0 {
                self.search(
                    &SearchRequest {
                        query: request.query.clone(),
                        mode: request.mode,
                        limit: topic_limit * 8,
                        scope: request.scope.clone(),
                        source_kind: Some("topic_summary".to_string()),
                    },
                    provider,
                )?
            } else {
                Vec::new()
            };
            let mut hits = self.search_without_topics(
                &SearchRequest {
                    query: request.query.clone(),
                    mode: request.mode,
                    limit: remaining_slots.saturating_mul(8).min(100),
                    scope: request.scope.clone(),
                    source_kind: None,
                },
                provider,
            )?;
            hits.retain(|hit| !matches!(hit.source_kind.as_str(), "memory" | "evidence"));
            let relevance =
                ReferenceRelevance::new(&request.query, hits.iter().chain(topic_hits.iter()));
            let rerank = |hits: &mut Vec<SearchHit>| {
                hits.retain_mut(|hit| {
                    if let Some(score) = relevance.score(hit) {
                        // Weighted overlap ranks partial evidence; original
                        // search order remains the stable tie breaker.
                        hit.score = score;
                        true
                    } else {
                        false
                    }
                });
                hits.sort_by(|left, right| right.score.total_cmp(&left.score));
            };
            // Topic summaries already address a whole question. Preserve the
            // lexical/hybrid retrieval order for them; clause normalisation is
            // useful for partial source evidence but can otherwise promote a
            // secondary topic above the best whole-question match.
            topic_hits.retain_mut(|hit| {
                if let Some(score) = relevance.score(hit) {
                    hit.score = score;
                    true
                } else {
                    false
                }
            });
            rerank(&mut hits);
            let best_topic_score = topic_hits.first().map_or(0.0, |hit| hit.score);
            let mut topics = Vec::new();
            let mut seen_topics = BTreeSet::new();
            let mut covered_terms = BTreeSet::new();
            for hit in topic_hits {
                // A weak secondary topic is discovery material, not a reason
                // to expand another graph branch into every answer packet.
                let terms = &relevance.matches[&hit.chunk_id];
                if hit.score < best_topic_score * 0.65
                    || terms.is_subset(&covered_terms)
                    || !seen_topics.insert(hit.document_id)
                {
                    continue;
                }
                if let Some(reference) = self.load_context_reference(hit)? {
                    covered_terms.extend(terms);
                    topics.push(reference);
                }
                if topics.len() >= topic_limit {
                    break;
                }
            }
            let best_source_score = hits.first().map_or(0.0, |hit| hit.score);
            let mut references = Vec::new();
            let mut positions = BTreeMap::new();
            for hit in hits {
                if hit.score < best_source_score * 0.5 || positions.contains_key(&hit.document_id) {
                    continue;
                }
                if let Some(reference) = self.load_context_reference(hit)? {
                    positions.insert(reference.document_id, references.len());
                    references.push(reference);
                }
            }
            // A reviewed topic establishes a relationship to its exact premises,
            // including premises that do not repeat the question's subject.
            // At most 32 unique source lookups, never recursive graph search.
            for topic in &topics {
                for mut reference in self.topic_source_references(topic, 16)? {
                    // Premises inherit most of their topic's relevance, while
                    // directly matched evidence can still outrank that bridge.
                    reference.relevance_score *= 0.8;
                    if let Some(&index) = positions.get(&reference.document_id) {
                        let existing: &mut ContextReference = &mut references[index];
                        existing.relevance_score =
                            existing.relevance_score.max(reference.relevance_score);
                    } else {
                        positions.insert(reference.document_id, references.len());
                        references.push(reference);
                    }
                }
            }
            references
                .sort_by(|left, right| right.relevance_score.total_cmp(&left.relevance_score));
            if references.len() > remaining_slots {
                packet.truncated = true;
            }
            // Reserve the actual selected source evidence first. A topic may
            // consume spare room or replace an exact duplicate factual quote;
            // it must never force a source to be shortened or discarded.
            for mut reference in references {
                if packet.references.len() >= remaining_slots {
                    break;
                }
                if push_reference_if_fits(&mut packet, reference.clone()) {
                    continue;
                }
                packet.truncated = true;
                if let Some(claim) = &mut reference.source_claim {
                    // Multiple supports can be inspected separately. Preserve
                    // the entire accepted statement, including its final caveat.
                    claim.citations.truncate(1);
                    let _ = push_reference_if_fits(&mut packet, reference);
                    continue;
                }
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
            let mut topic_chars = 0;
            for topic in topics {
                let mut candidate = packet.clone();
                let mut just_topic = packet.clone();
                just_topic.references = vec![topic.clone()];
                candidate
                    .references
                    .retain(|reference| !topic_already_quotes_reference(&just_topic, reference));
                if candidate.references.len() >= remaining_slots {
                    packet.truncated = true;
                    continue;
                }
                if push_topic_if_fits(&mut candidate, topic, &mut topic_chars) {
                    packet = candidate;
                } else {
                    packet.truncated = true;
                }
            }
        }
        packet.used_chars = packet.render_markdown().chars().count();
        Ok(packet)
    }

    fn load_context_reference(&self, hit: SearchHit) -> Result<Option<ContextReference>> {
        let row = self
            .connection
            // Cache the SQL program, never its result: every candidate still
            // checks current source/topic validity when the statement executes.
            .prepare_cached(
                "SELECT d.body, c.start_byte
             FROM chunks c
             JOIN documents d ON d.id = c.document_id
             WHERE c.id = ?1 AND c.document_id = ?2 AND d.active = 1
               AND (d.source_kind <> 'source_claim' OR EXISTS (
                   SELECT 1 FROM source_valid_claims sc WHERE sc.document_id = d.id
               ))
               AND (d.source_kind <> 'topic_summary' OR EXISTS (
                   SELECT 1 FROM topic_valid_documents tv WHERE tv.document_id = d.id
               ))",
            )?
            .query_row(params![hit.chunk_id, hit.document_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
            })
            .optional()?;
        let Some((body, approximate_start)) = row else {
            // A concurrent withdrawal may retire a hit after search returns.
            return Ok(None);
        };
        let topic = if hit.source_kind == "topic_summary" {
            let Some(topic) = self.topic_context(hit.document_id)? else {
                return Ok(None);
            };
            Some(topic)
        } else {
            None
        };
        let (start_byte, end_byte) = if topic.is_some() || hit.source_kind == "source_claim" {
            (0, body.len())
        } else {
            exact_reference_range(&body, &hit.content, approximate_start)?
        };
        let source_claim = if hit.source_kind == "source_claim" {
            let Some(mut claim) = self.source_claim_context(hit.document_id, 3)? else {
                return Ok(None);
            };
            if claim.citations.is_empty() {
                return Ok(None);
            }
            for citation in &mut claim.citations {
                align_source_citation(citation, start_byte, end_byte, &body)?;
            }
            Some(claim)
        } else {
            None
        };
        let reference = ContextReference {
            document_id: hit.document_id,
            chunk_id: hit.chunk_id,
            source_uri: hit.source_uri,
            source_kind: hit.source_kind,
            scope: hit.scope,
            title: hit.title,
            content: if topic.is_some() || source_claim.is_some() {
                body
            } else {
                hit.content
            },
            relevance_score: hit.score,
            start_byte,
            end_byte,
            source_claim,
            topic,
        };
        Ok(Some(reference))
    }

    fn topic_source_references(
        &self,
        reference: &ContextReference,
        limit: usize,
    ) -> Result<Vec<ContextReference>> {
        let Some(topic) = &reference.topic else {
            return Ok(Vec::new());
        };
        let mut references = Vec::new();
        let mut seen = BTreeSet::new();
        // Join the topic's authoritative validity view again: withdrawal or
        // expiry between topic retrieval and expansion must not admit premises.
        let mut statement = self.connection.prepare_cached(
            "SELECT d.id,c.id,d.source_uri,d.source_kind,d.scope,d.title,d.body
             FROM source_valid_supports s
             JOIN source_valid_claims v ON v.claim_id=s.claim_id
             JOIN documents d ON d.id=v.document_id
             JOIN chunks c ON c.document_id=d.id
             WHERE s.source_id=?1 AND s.revision_id=?2
               AND s.start_byte=?3 AND s.end_byte=?4 AND d.body=?5
               AND d.scope=?6 AND d.active=1 AND d.source_kind='source_claim'
               AND EXISTS(SELECT 1 FROM topic_valid_documents
                          WHERE revision_id=?7 AND document_id=?8)
             ORDER BY c.id LIMIT 1",
        )?;
        for citation in topic.statements.iter().flat_map(|block| &block.citations) {
            let key = (
                &citation.revision_id,
                citation.start_byte,
                citation.end_byte,
            );
            if !seen.insert(key) {
                continue;
            }
            if seen.len() > limit {
                break;
            }
            let hit = statement
                .query_row(
                    params![
                        citation.source_id,
                        citation.revision_id,
                        citation.start_byte as i64,
                        citation.end_byte as i64,
                        citation.quote,
                        reference.scope,
                        topic.revision_id,
                        reference.document_id
                    ],
                    |row| {
                        Ok(SearchHit {
                            document_id: row.get(0)?,
                            chunk_id: row.get(1)?,
                            source_uri: row.get(2)?,
                            source_kind: row.get(3)?,
                            scope: row.get(4)?,
                            title: row.get(5)?,
                            content: row.get(6)?,
                            score: reference.relevance_score,
                            lexical_rank: None,
                            vector_rank: None,
                            vector_distance: None,
                        })
                    },
                )
                .optional()?;
            if let Some(hit) = hit
                && let Some(source) = self.load_context_reference(hit)?
            {
                references.push(source);
                if references.len() >= limit {
                    break;
                }
            }
        }
        Ok(references)
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
                        m.importance, m.confidence, m.pinned, m.observed_at_ms,
                        m.last_confirmed_at_ms, m.valid_until_ms,
                        EXISTS(SELECT 1 FROM learning_reviews r WHERE r.document_id=d.id
                            OR (r.document_id IS NULL AND r.scope=d.scope AND r.canonical_key=m.canonical_key))
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
                        observed_at_ms: row.get(9)?,
                        last_confirmed_at_ms: row.get(10)?,
                        valid_until_ms: row.get(11)?,
                        review_required: row.get(12)?,
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

pub(crate) fn distill_in_transaction(
    transaction: &Transaction<'_>,
    mut input: DistillInput,
) -> Result<DistillOutcome> {
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
    let evidence = load_evidence(transaction, &input.evidence_session_id)?;
    if evidence.scope != input.scope {
        anyhow::bail!("evidence and memory must have the same scope");
    }
    if input
        .valid_until_ms
        .is_some_and(|until| until <= evidence.completed_at_ms)
    {
        anyhow::bail!("valid_until_ms must be later than the evidence observation time");
    }
    let citation = locate_citation(&evidence.body, redacted_quote.value.trim())?;
    let content_hash = sha256_hex(&redacted_content.value);

    // An alias names the same claim; revisions retain its original canonical key.
    if let Some(key) = transaction.query_row(
        "SELECT m.canonical_key FROM memory_aliases a JOIN memory_items m ON m.document_id = a.document_id WHERE a.canonical_key = ?1",
        [&input.canonical_key], |row| row.get::<_, String>(0),
    ).optional()? {
        input.canonical_key = key;
    }

    let head = transaction
        .query_row(
            "SELECT h.document_id, d.content_hash, d.scope, m.memory_kind, m.last_confirmed_at_ms
             FROM memory_heads h
             JOIN documents d ON d.id = h.document_id
             JOIN memory_items m ON m.document_id = h.document_id
             WHERE h.canonical_key = ?1
               AND d.active = 1
               AND m.superseded_by IS NULL",
            [&input.canonical_key],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                ))
            },
        )
        .optional()?;

    if let Some((_, _, scope, kind, _)) = &head
        && (scope != &input.scope || kind != &input.memory_kind)
    {
        anyhow::bail!("canonical key already belongs to a different scope or memory kind");
    }
    // Exact duplicate content cannot acquire a second active identity merely by
    // choosing a new key. Fuzzy meaning is deliberately not merged here.
    let duplicate = if head.is_none() && input.supersedes.is_none() {
        transaction.query_row(
            "SELECT m.document_id, m.canonical_key FROM memory_items m JOIN documents d ON d.id = m.document_id
             WHERE d.active = 1 AND d.scope = ?1 AND m.memory_kind = ?2 AND d.content_hash = ?3
               AND m.superseded_by IS NULL AND m.canonical_key IS NOT NULL ORDER BY m.document_id LIMIT 1",
            params![input.scope, input.memory_kind, content_hash],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        ).optional()?
    } else {
        None
    };
    if let Some((document_id, key)) = duplicate {
        transaction.execute("INSERT INTO memory_aliases(canonical_key, document_id, created_at_ms) VALUES(?1, ?2, ?3)", params![input.canonical_key, document_id, now_ms()])?;
        input.canonical_key = key;
        return distill_in_transaction(transaction, input);
    }

    if let Some((document_id, existing_hash, _, _, _)) = head.as_ref()
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
                 last_confirmed_at_ms = max(coalesce(last_confirmed_at_ms, ?5), ?5),
                 valid_until_ms = CASE WHEN ?5 >= coalesce(last_confirmed_at_ms, ?5)
                                      THEN coalesce(?6, valid_until_ms) ELSE valid_until_ms END
             WHERE document_id = ?1",
            params![
                document_id,
                input.importance,
                input.confidence,
                input.pinned,
                evidence.completed_at_ms,
                input.valid_until_ms,
            ],
        )?;
        insert_citation(
            transaction,
            *document_id,
            &evidence,
            &citation,
            redacted_quote.value.trim(),
        )?;
        sync_memory_indexes(transaction, *document_id)?;
        let evidence_count = citation_count(transaction, *document_id)?;
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
        Some((document_id, _, _, _, confirmed_at)) => {
            if input.supersedes != Some(document_id) {
                anyhow::bail!(
                    "memory `{}` already has different active content in document {document_id}; pass --supersedes {document_id} after review",
                    input.canonical_key
                );
            }
            if confirmed_at.is_some_and(|confirmed| evidence.completed_at_ms <= confirmed) {
                anyhow::bail!("supersession requires evidence newer than the current confirmation");
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
    let nonce = random_nonce(transaction)?;
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
    let outcome = ingest_prepared(transaction, prepared)?;
    transaction.execute(
        "INSERT INTO memory_items(
             document_id, memory_kind, importance, confidence, valid_from_ms, pinned,
             canonical_key, last_confirmed_at_ms, observed_at_ms, valid_until_ms
         ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, NULL, ?5, ?5, ?7)",
        params![
            outcome.document_id,
            input.memory_kind,
            input.importance,
            input.confidence,
            evidence.completed_at_ms,
            input.pinned,
            input.valid_until_ms,
        ],
    )?;
    if let Some(previous_document_id) = superseded_document_id {
        let updated = transaction.execute(
            "UPDATE memory_items
             SET superseded_by = ?2, valid_until_ms = CASE
                 WHEN valid_until_ms IS NULL THEN ?3 ELSE min(valid_until_ms, ?3) END
             WHERE document_id = ?1 AND superseded_by IS NULL",
            params![
                previous_document_id,
                outcome.document_id,
                evidence.completed_at_ms
            ],
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
    if let Some(previous_document_id) = superseded_document_id {
        transaction.execute(
            "UPDATE memory_aliases SET document_id = ?2 WHERE document_id = ?1",
            params![previous_document_id, outcome.document_id],
        )?;
    }
    transaction.execute(
        "INSERT INTO memory_heads(canonical_key, document_id, updated_at_ms)
         VALUES(?1, ?2, ?3)
         ON CONFLICT(canonical_key) DO UPDATE SET
             document_id = excluded.document_id,
             updated_at_ms = excluded.updated_at_ms",
        params![input.canonical_key, outcome.document_id, now_ms()],
    )?;
    insert_citation(
        transaction,
        outcome.document_id,
        &evidence,
        &citation,
        redacted_quote.value.trim(),
    )?;
    sync_memory_indexes(transaction, outcome.document_id)?;
    let evidence_count = citation_count(transaction, outcome.document_id)?;

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

pub(crate) fn sync_memory_indexes(transaction: &Transaction<'_>, document_id: i64) -> Result<()> {
    let at_ms = now_ms();
    let eligible = transaction.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM documents d JOIN memory_items m ON m.document_id = d.id
             WHERE d.id = ?1 AND d.active = 1 AND m.superseded_by IS NULL
               AND (m.valid_until_ms IS NULL OR m.valid_until_ms > ?2)
         )",
        params![document_id, at_ms],
        |row| row.get::<_, bool>(0),
    )?;
    if !eligible {
        // Historical observations still keep their immutable content and
        // citations, but must not schedule work for an already expired claim.
        transaction.execute(
            "DELETE FROM embedding_queue WHERE chunk_id IN (
                 SELECT id FROM chunks WHERE document_id = ?1
             )",
            [document_id],
        )?;
        return Ok(());
    }

    // A fresh confirmation can revive an unchanged claim after index rebuild
    // removed its expired cache. Reuse any surviving vectors and leases.
    transaction.execute(
        "INSERT INTO chunk_fts(rowid, title, content, source_uri, scope, source_kind)
         SELECT c.id, d.title, c.content, d.source_uri, d.scope, d.source_kind
         FROM chunks c JOIN documents d ON d.id = c.document_id
         WHERE d.id = ?1 AND NOT EXISTS(
             SELECT 1 FROM chunk_fts WHERE rowid = c.id
         )",
        [document_id],
    )?;
    transaction.execute(
        "INSERT OR IGNORE INTO embedding_queue(chunk_id, queued_at_ms, priority)
         SELECT id, ?2, 100 FROM chunks
         WHERE document_id = ?1 AND embedding_model IS NULL",
        params![document_id, at_ms],
    )?;
    Ok(())
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

/// Reference evidence can support one clause without repeating every other
/// clause. Entity anchors still have to occur in the evidence or its exact
/// scoped collection. This deliberately does not change personal-memory recall.
struct ReferenceRelevance {
    scores: BTreeMap<i64, f64>,
    matches: BTreeMap<i64, BTreeSet<usize>>,
}

impl ReferenceRelevance {
    fn new<'a>(query: &str, hits: impl Iterator<Item = &'a SearchHit>) -> Self {
        let query = query.chars().take(2048).collect::<String>();
        let raw_terms = normalized_terms(&query);
        let greeting = raw_terms.len() <= 3
            && raw_terms
                .first()
                .is_some_and(|term| matches!(term.as_str(), "hi" | "hello" | "hey"));
        let terms = if greeting {
            Vec::new()
        } else {
            reference_terms(&query)
        };
        let anchors = query
            .split(|c: char| !c.is_alphanumeric())
            .filter(|term| {
                term.len() >= 3
                    && term.is_ascii()
                    && (term.chars().all(|c| c.is_numeric())
                        || term.chars().next().is_some_and(char::is_uppercase))
            })
            .map(normalize_term)
            .filter(|term| terms.contains(term))
            .collect::<Vec<_>>();
        let mut matches = BTreeMap::new();
        let mut missing_anchors = BTreeMap::new();
        let mut documents = BTreeSet::new();
        let mut frequencies = vec![BTreeSet::new(); terms.len()];
        for hit in hits {
            let content = reference_terms(&format!(
                "{}\n{}",
                hit.title.as_deref().unwrap_or_default(),
                hit.content,
            ));
            let scope = normalized_terms(&hit.scope);
            if !anchors.is_empty()
                && !anchors.iter().any(|anchor| {
                    scope.contains(anchor) || content.iter().any(|term| terms_match(anchor, term))
                })
            {
                continue;
            }
            // A collection name is not a substitute for a different named
            // subject in the same clause (Cedar must not stand in for Einstein).
            // Keep this clause-local so an English operation name in a second
            // question does not exclude evidence for the first Chinese one.
            let missing = terms
                .iter()
                .enumerate()
                .filter_map(|(index, term)| {
                    (anchors.contains(term)
                        && !scope.contains(term)
                        && !content.iter().any(|word| terms_match(term, word)))
                    .then_some(index)
                })
                .collect::<BTreeSet<_>>();
            missing_anchors.insert(hit.chunk_id, missing);
            documents.insert(hit.document_id);
            let matched = terms
                .iter()
                .enumerate()
                .filter_map(|(index, term)| {
                    content
                        .iter()
                        .any(|other| terms_match(term, other))
                        .then_some(index)
                })
                .collect::<BTreeSet<_>>();
            for &index in &matched {
                frequencies[index].insert(hit.document_id);
            }
            matches.insert(hit.chunk_id, matched);
        }
        // Local inverse document frequency discounts project names and common
        // boilerplate shared by nearly every candidate. Unmatched question
        // wording contributes no denominator penalty to a partial source.
        let weights = frequencies
            .iter()
            .map(|documents_with_term| {
                if documents_with_term.is_empty() {
                    0.0
                } else {
                    ((documents.len() + 1) as f64 / (documents_with_term.len() + 1) as f64).ln()
                        + 1.0
                }
            })
            .collect::<Vec<_>>();
        let total: f64 = weights.iter().sum();
        let clauses = reference_clauses(&query, &terms, &anchors);
        let mut raw_scores = BTreeMap::new();
        let mut best = vec![0.0f64; clauses.len()];
        for (&id, matched) in &matches {
            let raw = clauses
                .iter()
                .enumerate()
                .map(|(index, clause)| {
                    let overlap = clause.intersection(matched).copied().collect::<Vec<_>>();
                    let topical_matches = overlap
                        .iter()
                        .filter(|&&index| !anchors.contains(&terms[index]))
                        .count();
                    let score = if !clause.is_disjoint(&missing_anchors[&id])
                        || matched.len() < terms.len().min(2)
                        || overlap.len() < clause.len().min(2)
                        // In a rich question, the project name plus one generic
                        // word is insufficient, even with no stronger candidate.
                        || (clause.len() >= 4 && topical_matches < 2)
                        || (overlap.len() as f64 / clause.len() as f64) < 0.25
                    {
                        0.0
                    } else {
                        let denominator: f64 = clause.iter().map(|&term| weights[term]).sum();
                        if denominator > 0.0 {
                            overlap.iter().map(|&term| weights[term]).sum::<f64>() / denominator
                        } else {
                            0.0
                        }
                    };
                    best[index] = best[index].max(score);
                    score
                })
                .collect::<Vec<_>>();
            raw_scores.insert(id, raw);
        }
        // Balance clauses before comparing candidates. Otherwise a long CJK
        // clause with many bigrams can suppress the sole qualifier answering a
        // shorter English clause, despite room for both original statements.
        let scores = raw_scores
            .into_iter()
            .filter_map(|(id, raw)| {
                let balanced = raw
                    .iter()
                    .zip(&best)
                    .map(
                        |(&score, &best)| {
                            if best > 0.0 { score / best } else { 0.0 }
                        },
                    )
                    .fold(0.0f64, f64::max);
                if balanced == 0.0 || total == 0.0 {
                    None
                } else {
                    let coverage: f64 = matches[&id].iter().map(|&term| weights[term]).sum();
                    Some((id, balanced + 0.05 * coverage / total))
                }
            })
            .collect();
        Self { scores, matches }
    }

    fn score(&self, hit: &SearchHit) -> Option<f64> {
        self.scores.get(&hit.chunk_id).copied()
    }
}

fn reference_clauses(query: &str, terms: &[String], anchors: &[String]) -> Vec<BTreeSet<usize>> {
    let mut clauses = vec![(0..terms.len()).collect()];
    // A code-switched request may name an English operation and describe its
    // condition in Chinese. Evaluate the shared English terms as a language
    // facet too; Chinese bigrams must not dilute those identifiers away.
    let latin = terms
        .iter()
        .enumerate()
        .filter_map(|(index, term)| term.is_ascii().then_some(index))
        .collect::<BTreeSet<_>>();
    if latin.len() >= 2 && latin.len() < terms.len() {
        clauses.push(latin);
    }
    let separated = query
        .to_lowercase()
        .replace(" and ", "\n")
        .replace(" or ", "\n");
    for clause in separated
        .split(['?', '？', ';', '；', ',', '，', '\n'])
        .take(16)
    {
        let words = reference_terms(clause);
        let indices = terms
            .iter()
            .enumerate()
            .filter_map(|(index, term)| words.contains(term).then_some(index))
            .collect::<BTreeSet<_>>();
        // A named comparison subject can be a fragment ("Compare Ada and
        // Grace compilers"). It still needs a second global topical match.
        // Generic fragments such as "and history" cannot admit a hit alone.
        let named_fragment =
            indices.len() == 1 && indices.iter().any(|&index| anchors.contains(&terms[index]));
        if (indices.len() >= 2 || named_fragment) && !clauses.contains(&indices) {
            clauses.push(indices);
        }
    }
    clauses
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x3400..=0x9fff | 0xf900..=0xfaff | 0x20000..=0x2fa1f)
}

fn reference_terms(value: &str) -> Vec<String> {
    // Generic question/request words are neither subjects nor evidence. CJK
    // bigrams permit overlap inside an unsegmented Chinese question; no fuzzy
    // edit matching is used for these two-character terms.
    const QUESTION_WORDS: &[&str] = &[
        "describe",
        "explain",
        "should",
        "show",
        "tell",
        "why",
        "summarise",
        "summarize",
        "compare",
    ];
    let mut terms = value
        .split(|c: char| !c.is_alphanumeric())
        .filter(|term| !term.chars().any(is_cjk))
        .map(str::to_lowercase)
        // Filter before stemming: "does" otherwise becomes "doe" and can
        // accidentally act as a named-entity anchor in a natural question.
        .filter(|term| !CONTEXT_STOP_WORDS.contains(&term.as_str()))
        .filter(|term| !QUESTION_WORDS.contains(&term.as_str()))
        .map(|term| normalize_term(&term))
        .filter(|term| term.chars().count() >= 3)
        .collect::<Vec<_>>();
    for run in value.split(|c| !is_cjk(c)) {
        let chars = run.chars().collect::<Vec<_>>();
        for pair in chars.windows(2) {
            terms.push(pair.iter().collect());
        }
    }
    terms.sort();
    terms.dedup();
    terms.truncate(256);
    terms
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

const CONTEXT_STOP_WORDS: &[&str] = &[
    "about", "after", "again", "also", "and", "are", "can", "could", "does", "for", "from", "have",
    "hello", "help", "how", "into", "just", "like", "more", "please", "recall", "remember", "that",
    "the", "their", "then", "there", "this", "those", "use", "what", "when", "where", "which",
    "with", "would", "you", "your",
];

fn meaningful_terms(value: &str) -> Vec<String> {
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
        .filter(|term| !CONTEXT_STOP_WORDS.contains(&term.as_str()))
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

pub(crate) fn load_evidence(
    transaction: &Transaction<'_>,
    session_id: &str,
) -> Result<EvidenceRecord> {
    transaction
        .query_row(
            "SELECT e.id, e.document_id, d.body, d.scope, e.completed_at_ms
             FROM evidence_sessions e
             JOIN documents d ON d.id = e.document_id
             WHERE e.session_id = ?1 AND d.active = 1",
            [session_id],
            |row| {
                Ok(EvidenceRecord {
                    id: row.get(0)?,
                    document_id: row.get(1)?,
                    body: row.get(2)?,
                    scope: row.get(3)?,
                    completed_at_ms: row.get(4)?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| anyhow::anyhow!("evidence session `{session_id}` was not recorded"))
}

#[derive(Debug)]
pub(crate) struct EvidenceRecord {
    pub(crate) id: i64,
    pub(crate) document_id: i64,
    pub(crate) body: String,
    pub(crate) scope: String,
    pub(crate) completed_at_ms: i64,
}

#[derive(Debug)]
pub(crate) struct CitationLocation {
    start_byte: usize,
    end_byte: usize,
    start_line: usize,
    end_line: usize,
}

pub(crate) fn locate_citation(body: &str, quote: &str) -> Result<CitationLocation> {
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

pub(crate) fn insert_citation(
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

fn push_topic_if_fits(
    packet: &mut ContextPacket,
    reference: ContextReference,
    topic_chars: &mut usize,
) -> bool {
    let chars = crate::model::render_context_reference(&reference)
        .chars()
        .count();
    if topic_chars.saturating_add(chars) > packet.max_chars / 4 {
        return false;
    }
    if push_reference_if_fits(packet, reference) {
        *topic_chars += chars;
        true
    } else {
        false
    }
}

fn topic_already_quotes_reference(packet: &ContextPacket, reference: &ContextReference) -> bool {
    let Some(claim) = &reference.source_claim else {
        return false;
    };
    packet
        .references
        .iter()
        .filter_map(|reference| reference.topic.as_ref())
        .any(|topic| {
            topic.statements.iter().any(|statement| {
                statement.kind == "fact"
                    && statement.text == reference.content
                    && statement.citations.len() == claim.citations.len()
                    && statement.citations.iter().all(|citation| {
                        claim.citations.iter().any(|other| {
                            citation.source_id == other.source_id
                                && citation.revision_id == other.revision_id
                                && citation.source_kind == other.source_kind
                                && citation.start_byte == other.start_byte
                                && citation.end_byte == other.end_byte
                                && citation.quote == other.quote
                        })
                    })
            })
        })
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
    if let Some(claim) = &mut reference.source_claim {
        for citation in &mut claim.citations {
            truncate_source_citation_quote(citation, max_chars);
            debug_assert_eq!(citation.quote, reference.content);
        }
    }
}

fn align_source_citation(
    citation: &mut SourceClaimCitation,
    claim_start: usize,
    claim_end: usize,
    excerpt: &str,
) -> Result<()> {
    anyhow::ensure!(
        citation.quote.get(claim_start..claim_end) == Some(excerpt),
        "retrieved source statement does not match its supporting source range"
    );
    citation.start_line += citation.quote[..claim_start].matches('\n').count();
    citation.start_byte += claim_start;
    citation.quote = excerpt.to_owned();
    truncate_source_citation_quote(citation, usize::MAX);
    Ok(())
}

fn truncate_citation_quote(citation: &mut ContextCitation, max_chars: usize) {
    truncate_exact_prefix(&mut citation.quote, max_chars);
    citation.end_byte = citation.start_byte + citation.quote.len();
    citation.end_line = citation.start_line + citation.quote.matches('\n').count();
}

fn truncate_source_citation_quote(citation: &mut SourceClaimCitation, max_chars: usize) {
    truncate_exact_prefix(&mut citation.quote, max_chars);
    citation.end_byte = citation.start_byte + citation.quote.len();
    // Source ranges name the line containing the final included byte. A
    // trailing newline belongs to that line, not the following empty line.
    citation.end_line = citation.start_line
        + citation
            .quote
            .strip_suffix('\n')
            .unwrap_or(&citation.quote)
            .matches('\n')
            .count();
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
            source_claim: None,
            topic: None,
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

    #[test]
    fn truncated_source_citation_keeps_original_exact_coordinates() {
        let mut citation = crate::model::SourceClaimCitation {
            source_id: "source".into(),
            source_key: "key".into(),
            revision_id: "revision".into(),
            source_kind: "article".into(),
            origin: None,
            title: None,
            author: None,
            published_at_ms: None,
            captured_at_ms: 1,
            start_byte: 20,
            end_byte: 50,
            start_line: 3,
            end_line: 5,
            quote: "Moon 🌙\nuses SQLite\n".into(),
        };
        super::truncate_source_citation_quote(&mut citation, 7);
        assert_eq!(citation.quote, "Moon 🌙\n");
        assert_eq!(citation.end_byte, 20 + citation.quote.len());
        assert_eq!(citation.end_line, 3);
        assert_eq!(citation.source_id, "source");
        assert_eq!(citation.revision_id, "revision");
    }

    #[test]
    fn source_citation_tracks_a_later_chunk_and_each_context_budget_cut() {
        let prefix = "earlier source statement\n".repeat(80);
        let excerpt = "🌙 Later source evidence\ncontinues with exact wording.";
        let claim = format!("{prefix}{excerpt}\nlast statement");
        let source = format!("Source heading\n{claim}");
        let source_start = "Source heading\n".len();
        let mut citation = crate::model::SourceClaimCitation {
            source_id: "source".into(),
            source_key: "key".into(),
            revision_id: "revision".into(),
            source_kind: "reference".into(),
            origin: None,
            title: None,
            author: None,
            published_at_ms: None,
            captured_at_ms: 1,
            start_byte: source_start,
            end_byte: source_start + claim.len(),
            start_line: 2,
            end_line: 83,
            quote: claim,
        };
        super::align_source_citation(
            &mut citation,
            prefix.len(),
            prefix.len() + excerpt.len(),
            excerpt,
        )
        .expect("align later chunk");
        assert_eq!(citation.quote, excerpt);
        assert_eq!(citation.start_line, 82);
        assert_eq!(citation.end_line, 83);
        let mut reference = ContextReference {
            document_id: 1,
            chunk_id: 2,
            source_uri: "moon-source-claim://claim".into(),
            source_kind: "source_claim".into(),
            scope: "moon".into(),
            title: None,
            content: excerpt.into(),
            relevance_score: 1.0,
            start_byte: prefix.len(),
            end_byte: prefix.len() + excerpt.len(),
            source_claim: Some(crate::model::SourceClaimContext {
                claim_id: "claim".into(),
                kind: "author_statement".into(),
                citations: vec![citation],
            }),
            topic: None,
        };
        for max_chars in [500, 30, 24, 4] {
            truncate_reference_content(&mut reference, max_chars);
            let citation = &reference.source_claim.as_ref().unwrap().citations[0];
            assert_eq!(citation.quote, reference.content);
            assert_eq!(
                &source[citation.start_byte..citation.end_byte],
                reference.content
            );
            assert_eq!(
                citation.start_line,
                1 + source[..citation.start_byte].matches('\n').count()
            );
            assert_eq!(
                citation.end_line,
                1 + source.as_bytes()[..citation.end_byte - 1]
                    .iter()
                    .filter(|&&byte| byte == b'\n')
                    .count()
            );
        }
    }

    fn topic_reference_fixture() -> ContextReference {
        let text = "Withdrawal is not deletion; the retained source remains available for review.";
        ContextReference {
            document_id: 1,
            chunk_id: 2,
            source_uri: "moon-topic://topic".into(),
            source_kind: "topic_summary".into(),
            scope: "moon".into(),
            title: None,
            content: text.into(),
            relevance_score: 1.0,
            start_byte: 0,
            end_byte: text.len(),
            source_claim: None,
            topic: Some(crate::topics::TopicContext {
                topic_id: "topic".into(),
                key: "lifecycle".into(),
                revision_id: "topic-revision".into(),
                review_complete: false,
                statements: vec![crate::topics::TopicContextStatement {
                    kind: "fact".into(),
                    text: text.into(),
                    input_ids: vec!["claim".into()],
                    citations: vec![crate::model::SourceClaimCitation {
                        source_id: "source".into(),
                        source_key: "policy".into(),
                        revision_id: "revision".into(),
                        source_kind: "reference".into(),
                        origin: None,
                        title: None,
                        author: None,
                        published_at_ms: None,
                        captured_at_ms: 1,
                        start_byte: 10,
                        end_byte: 10 + text.len(),
                        start_line: 2,
                        end_line: 2,
                        quote: text.into(),
                    }],
                }],
            }),
        }
    }

    #[test]
    fn topic_budget_omits_whole_blocks_instead_of_cutting_qualifications() {
        let reference = topic_reference_fixture();
        let rendered_chars = crate::model::render_context_reference(&reference)
            .chars()
            .count();
        let mut packet = crate::ContextPacket {
            query: "withdrawal".into(),
            scope: Some("moon".into()),
            max_chars: rendered_chars * 4 - 1,
            used_chars: 0,
            truncated: false,
            memories: Vec::new(),
            references: Vec::new(),
        };
        let mut topic_chars = 0;
        assert!(!super::push_topic_if_fits(
            &mut packet,
            reference.clone(),
            &mut topic_chars
        ));
        assert!(packet.references.is_empty());
        assert_eq!(topic_chars, 0);
        packet.max_chars = rendered_chars * 4;
        assert!(super::push_topic_if_fits(
            &mut packet,
            reference.clone(),
            &mut topic_chars
        ));
        assert_eq!(topic_chars, rendered_chars);
        assert!(packet.render_markdown().contains(&reference.content));
        assert!(!super::push_topic_if_fits(
            &mut packet,
            reference,
            &mut topic_chars
        ));
        assert_eq!(packet.references.len(), 1);
    }

    #[test]
    fn topic_dedup_requires_exact_fact_wording_and_the_same_source_support() {
        let topic = topic_reference_fixture();
        let citations = topic.topic.as_ref().unwrap().statements[0]
            .citations
            .clone();
        let mut claim = topic.clone();
        claim.topic = None;
        claim.source_kind = "source_claim".into();
        claim.source_claim = Some(crate::model::SourceClaimContext {
            claim_id: "claim".into(),
            kind: "author_statement".into(),
            citations,
        });
        let mut packet = crate::ContextPacket {
            query: "withdrawal".into(),
            scope: Some("moon".into()),
            max_chars: 10_000,
            used_chars: 0,
            truncated: false,
            memories: Vec::new(),
            references: vec![topic],
        };
        assert!(super::topic_already_quotes_reference(&packet, &claim));
        claim.content.push_str(" Different qualification.");
        assert!(!super::topic_already_quotes_reference(&packet, &claim));
        claim.content = packet.references[0].content.clone();
        claim.source_claim.as_mut().unwrap().citations[0].revision_id = "other-revision".into();
        assert!(!super::topic_already_quotes_reference(&packet, &claim));
        claim.source_claim.as_mut().unwrap().citations[0].revision_id = "revision".into();
        packet.references[0].topic.as_mut().unwrap().statements[0].kind = "interpretation".into();
        assert!(!super::topic_already_quotes_reference(&packet, &claim));
    }
}
