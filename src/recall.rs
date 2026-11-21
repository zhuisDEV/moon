//! Candidate retrieval for an authenticated OpenClaw turn. The host supplies the
//! audience and source grant. The model can choose queries and judge relevance.
use crate::memory::load_evidence;
use crate::{EmbeddingProvider, SearchMode, SearchRequest, Store};
use anyhow::{Result, ensure};
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const MAX_PREVIOUS: usize = 2;
const MAX_CITATIONS: usize = 2;
const EXCERPT_CHARS: usize = 500;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecallQuery {
    pub text: String,
    pub temporal: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecallRequest {
    pub queries: Vec<RecallQuery>,
    pub audience: Vec<String>,
    pub mode: SearchMode,
    pub limit: usize,
    pub max_chars: usize,
    #[serde(default)]
    pub allow_originals: bool,
    #[serde(default = "observe_by_default")]
    pub observe: bool,
}

fn observe_by_default() -> bool {
    true
}

#[derive(Debug, Serialize)]
pub struct RecallPacket {
    pub request_id: Option<String>,
    pub candidates: Vec<RecallCandidate>,
    pub public_only: bool,
    /// Number of claims, including previous revisions.
    pub memory_count: usize,
    pub reference_count: usize,
    /// Size of the entire JSON candidate array, including escaped quotes/keys.
    pub packet_chars: usize,
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
pub struct RecallCandidate {
    #[serde(flatten)]
    pub claim: RecallClaim,
    pub matched_queries: Vec<usize>,
    /// Originals are authorised for the strongest matching query or queries.
    /// A tie with any current-only query keeps this record wording-only.
    pub temporal: bool,
    pub previous: Vec<RecallClaim>,
}

#[derive(Debug, Serialize)]
pub struct RecallClaim {
    pub document_id: i64,
    pub content: String,
    pub observed_at_ms: Option<i64>,
    /// Verified source, who may differ from the subject of the claim.
    pub verified_sender: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub review_required: bool,
    pub citations: Vec<RecallCitation>,
}

#[derive(Debug, Serialize)]
pub struct RecallCitation {
    pub verified_sender: String,
    pub completed_at_ms: i64,
    pub quote: String,
    /// Nearby original words from this same speaker, excluding assistant text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker_excerpt: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub excerpt_truncated: bool,
}

impl Store {
    pub fn recall_candidates(
        &self,
        request: &RecallRequest,
        provider: Option<&dyn EmbeddingProvider>,
    ) -> Result<RecallPacket> {
        let retrieve = || self.assemble_candidates(request, provider);
        if !request.observe {
            return retrieve();
        }
        let (request_id, mut packet) = self.observe_retrieval(request.mode, retrieve, |p| {
            (
                p.memory_count,
                p.reference_count,
                p.packet_chars,
                p.truncated,
            )
        })?;
        packet.request_id = request_id;
        Ok(packet)
    }

    fn assemble_candidates(
        &self,
        request: &RecallRequest,
        provider: Option<&dyn EmbeddingProvider>,
    ) -> Result<RecallPacket> {
        ensure!(
            (1..=3).contains(&request.queries.len()),
            "expected 1 to 3 recall queries"
        );
        ensure!(
            request
                .queries
                .iter()
                .all(|q| !q.text.trim().is_empty() && q.text.chars().count() <= 512),
            "recall query must contain 1 to 512 characters"
        );
        ensure!(
            (1..=32).contains(&request.limit),
            "candidate limit must be 1 to 32"
        );
        ensure!(
            (512..=32_000).contains(&request.max_chars),
            "max_chars must be 512 to 32000"
        );
        let snapshot = self.connection.unchecked_transaction()?;
        let scopes = serde_json::to_string(&self.readable_memory_scopes(&request.audience)?)?;
        let mut rankings = Vec::new();
        let mut matches = BTreeMap::<i64, Vec<(usize, usize)>>::new();
        for (index, query) in request.queries.iter().enumerate() {
            let hits = self.search_inner(
                &SearchRequest {
                    query: query.text.clone(),
                    mode: request.mode,
                    // Overfetch chunks so one long claim cannot monopolise a query.
                    limit: (request.limit * 4).min(100),
                    scope: None,
                    source_kind: Some("memory".into()),
                },
                provider,
                false,
                Some(&scopes),
            )?;
            let mut unique = BTreeSet::new();
            let ranking = hits
                .into_iter()
                .map(|hit| hit.document_id)
                .filter(|id| unique.insert(*id))
                .take(request.limit + 1)
                .collect::<Vec<_>>();
            for (rank, id) in ranking.iter().enumerate() {
                matches.entry(*id).or_default().push((index, rank));
            }
            rankings.push(ranking.into_iter());
        }

        let mut packet = RecallPacket {
            request_id: None,
            candidates: Vec::new(),
            public_only: true,
            memory_count: 0,
            reference_count: 0,
            packet_chars: 2,
            truncated: false,
        };
        let mut considered = BTreeSet::new();
        loop {
            let mut this_round = Vec::new();
            for ranking in &mut rankings {
                if let Some(id) = ranking.find(|id| considered.insert(*id)) {
                    this_round.push(id);
                }
            }
            if this_round.is_empty() {
                break;
            }
            let mut timelines = Vec::new();
            for id in this_round {
                if packet.memory_count == request.limit {
                    packet.truncated = true;
                    break;
                }
                let matched = matches.remove(&id).expect("ranked candidate");
                let best_rank = matched.iter().map(|&(_, rank)| rank).min();
                // Vector search in a small corpus can return the same rows for
                // unrelated subqueries. A lower rank or a mixed best-rank tie
                // cannot lift a facts-only source restriction for this record.
                let temporal = request.allow_originals
                    && matched
                        .iter()
                        .filter(|&&(_, rank)| Some(rank) == best_rank)
                        .all(|&(i, _)| request.queries[i].temporal);
                let Some((mut claim, public)) =
                    current_claim(&snapshot, id, &scopes, request.max_chars)?
                else {
                    packet.truncated = true;
                    continue;
                };
                if temporal && claim.verified_sender.is_some() {
                    claim.citations = admitted_citations(&snapshot, &claim)?;
                }
                let mut candidate = RecallCandidate {
                    claim,
                    matched_queries: matched.into_iter().map(|(i, _)| i).collect(),
                    temporal,
                    previous: Vec::new(),
                };
                if packet.reserve(
                    &mut candidate,
                    !packet.candidates.is_empty(),
                    request.max_chars,
                    |c| c.claim.citations.pop().is_some(),
                )? {
                    if temporal && !candidate.claim.citations.is_empty() {
                        timelines.push(packet.candidates.len());
                    }
                    packet.public_only &= public;
                    packet.candidates.push(candidate);
                }
            }
            // Give every query its first current head before spending the common
            // budget on chronology. Then follow direct links, one hop at a time.
            for _ in 0..MAX_PREVIOUS {
                let mut extend = Vec::new();
                for index in timelines {
                    if packet.append_previous(&snapshot, index, request)? {
                        extend.push(index);
                    }
                }
                timelines = extend;
            }
            // Hitting this bound does not establish the start of the history.
            packet.truncated |= !timelines.is_empty();
            if packet.memory_count == request.limit {
                packet.truncated |= rankings
                    .iter()
                    .any(|rank| rank.as_slice().iter().any(|id| !considered.contains(id)));
                break;
            }
        }
        snapshot.commit()?;
        Ok(packet)
    }
}

impl RecallPacket {
    /// The empty array accounts for both brackets. Inserting either a complete
    /// flattened candidate or a revision in its pre-existing [] adds the
    /// serialised record and (if needed) one comma, independently of escaping.
    fn reserve<T: Serialize>(
        &mut self,
        record: &mut T,
        comma: bool,
        max_chars: usize,
        mut remove_optional_evidence: impl FnMut(&mut T) -> bool,
    ) -> Result<bool> {
        loop {
            let chars = serde_json::to_string(record)?.chars().count() + usize::from(comma);
            if self.packet_chars + chars <= max_chars {
                self.packet_chars += chars;
                self.memory_count += 1;
                return Ok(true);
            }
            self.truncated = true;
            if !remove_optional_evidence(record) {
                return Ok(false);
            }
        }
    }

    fn append_previous(
        &mut self,
        tx: &Transaction<'_>,
        index: usize,
        request: &RecallRequest,
    ) -> Result<bool> {
        let candidate = &self.candidates[index];
        let child = candidate
            .previous
            .last()
            .unwrap_or(&candidate.claim)
            .document_id;
        let comma = !candidate.previous.is_empty();
        let Some(mut prior) = previous_claim(tx, child, request.max_chars)? else {
            return Ok(false);
        };
        prior.citations = admitted_citations(tx, &prior)?;
        if prior.citations.is_empty() {
            return Ok(false);
        }
        if self.memory_count == request.limit {
            self.truncated = true;
            return Ok(false);
        }
        // A prior claim must retain a dated original, even on the last slot.
        let fits = self.reserve(&mut prior, comma, request.max_chars, |p| {
            p.citations.len() > 1 && p.citations.pop().is_some()
        })?;
        if fits {
            self.candidates[index].previous.push(prior);
        }
        Ok(fits)
    }
}

/// Recheck the selected ID against the complete audience view in the same
/// snapshot as ranking. Publication grants cover wording alone. Assigned
/// legacy heads can expose their saved wording to their attested owner.
fn current_claim(
    tx: &Transaction<'_>,
    id: i64,
    scopes: &str,
    max_chars: usize,
) -> Result<Option<(RecallClaim, bool)>> {
    Ok(tx
        .query_row(
            "SELECT d.body, m.observed_at_ms, s.sender, p.document_id IS NOT NULL AND
            EXISTS(SELECT 1 FROM privacy_audiences a WHERE a.kind='public' AND a.scope=d.scope),
            EXISTS(SELECT 1 FROM learning_reviews r WHERE r.document_id=d.id OR
              (r.document_id IS NULL AND r.scope=d.scope AND r.canonical_key=m.canonical_key)),
            EXISTS(SELECT 1 FROM privacy_legacy_documents l WHERE l.document_id=d.id)
         FROM chat_recallable_memories visible
         JOIN documents d ON d.id=visible.document_id AND d.source_kind='memory'
         JOIN memory_items m ON m.document_id=d.id AND m.identity_scope=d.scope
         LEFT JOIN memory_publications p ON p.document_id=d.id
         LEFT JOIN speaker_memory_scopes s ON s.scope=d.scope
         WHERE visible.document_id=?1 AND visible.scope IN (SELECT value FROM json_each(?2))
           AND length(d.body)<=?3",
            params![id, scopes, max_chars as i64],
            |row| {
                let public: bool = row.get(3)?;
                let wording_only = public || row.get::<_, bool>(5)?;
                Ok((
                    RecallClaim {
                        document_id: id,
                        content: row.get(0)?,
                        observed_at_ms: if wording_only { None } else { row.get(1)? },
                        verified_sender: if wording_only { None } else { row.get(2)? },
                        review_required: !wording_only && row.get(4)?,
                        citations: vec![],
                    },
                    public,
                ))
            },
        )
        .optional()?)
}

/// A supersession edge alone cannot confer access. Require both identities to
/// match the already verified child and never traverse an alias or a merge of
/// different canonical keys. Ambiguous forks have no determinate predecessor.
fn previous_claim(
    tx: &Transaction<'_>,
    child: i64,
    max_chars: usize,
) -> Result<Option<RecallClaim>> {
    let mut stmt = tx.prepare_cached(
        "SELECT m.document_id, CASE WHEN length(d.body)<=?2 THEN d.body END, m.observed_at_ms, s.sender,
                EXISTS(SELECT 1 FROM learning_reviews r WHERE r.document_id=d.id OR
                   (r.document_id IS NULL AND r.scope=d.scope AND r.canonical_key=m.canonical_key))
         FROM memory_items next JOIN memory_items m ON m.superseded_by=next.document_id
           AND m.canonical_key=next.canonical_key AND m.identity_scope=next.identity_scope
         JOIN documents d ON d.id=m.document_id AND d.scope=m.identity_scope
         JOIN speaker_memory_scopes s ON s.scope=d.scope
         WHERE next.document_id=?1 AND d.source_kind='memory' AND d.active=1
         ORDER BY m.document_id LIMIT 2",
    )?;
    let mut rows = stmt.query(params![child, max_chars as i64])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let Some(content) = row.get::<_, Option<String>>(1)? else {
        return Ok(None);
    };
    let claim = RecallClaim {
        document_id: row.get(0)?,
        content,
        observed_at_ms: row.get(2)?,
        verified_sender: row.get(3)?,
        review_required: row.get(4)?,
        citations: vec![],
    };
    Ok(if rows.next()?.is_some() {
        None
    } else {
        Some(claim)
    })
}

/// Verify every source, including those outside the display budget. An
/// operator-authored citation under a plausible scope is insufficient. The
/// immutable transport admission must independently agree with that scope's
/// verified sender and exact owner set, and the bytes must still be original.
fn admitted_citations(tx: &Transaction<'_>, claim: &RecallClaim) -> Result<Vec<RecallCitation>> {
    let Some(sender) = &claim.verified_sender else {
        return Ok(Vec::new());
    };
    let verified: bool = tx.query_row(
        "SELECT EXISTS(
          SELECT 1 FROM documents d JOIN speaker_memory_scopes s ON s.scope=d.scope
          JOIN privacy_audiences a ON a.scope=s.audience_scope AND a.kind='private'
          WHERE d.id=?1 AND s.sender=?2
            AND NOT EXISTS(SELECT 1 FROM privacy_legacy_documents l WHERE l.document_id=d.id)
            AND NOT EXISTS(SELECT 1 FROM memory_ineligible_reply_dependencies bad WHERE bad.memory_document_id=d.id)
            AND NOT EXISTS(SELECT 1 FROM privacy_recoveries r WHERE r.document_id=d.id)
            AND NOT EXISTS(SELECT 1 FROM historic_speaker_scopes h WHERE h.scope=d.scope)
            AND NOT EXISTS(SELECT 1 FROM memory_publications p WHERE p.document_id=d.id)
            AND NOT EXISTS(
              SELECT 1 FROM memory_citations c WHERE c.memory_document_id=d.id AND NOT EXISTS(
                SELECT 1 FROM evidence_sessions e
                JOIN documents ed ON ed.id=e.document_id
                JOIN evidence_admissions b ON b.evidence_id=e.id
                JOIN privacy_admissions t ON t.admission_key=b.admission_key
                WHERE e.id=c.evidence_session_id AND ed.id=c.evidence_document_id AND e.redactions=0
                  AND ed.active=1 AND ed.source_kind='evidence' AND ed.scope=d.scope
                  AND t.sender=s.sender AND t.owners_json=a.readers_json
                  AND NOT EXISTS(SELECT 1 FROM privacy_legacy_documents l WHERE l.document_id=ed.id)
                  AND substr(CAST(ed.body AS BLOB),c.start_byte+1,c.end_byte-c.start_byte)=CAST(c.quote AS BLOB)
              ))
        )", params![claim.document_id, sender], |row| row.get(0),
    )?;
    if !verified {
        return Ok(Vec::new());
    }
    let mut stmt = tx.prepare_cached(
        "SELECT e.session_id, c.quote FROM memory_citations c
         JOIN evidence_sessions e ON e.id=c.evidence_session_id
         WHERE c.memory_document_id=?1
         ORDER BY e.completed_at_ms IS ?2 DESC, e.completed_at_ms DESC, c.id
         LIMIT ?3",
    )?;
    let rows = stmt.query_map(
        params![
            claim.document_id,
            claim.observed_at_ms,
            MAX_CITATIONS as i64
        ],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )?;
    let mut citations = Vec::new();
    for row in rows {
        let (session, quote) = row?;
        let evidence = load_evidence(tx, &session)?;
        let Some(original) = evidence.message_parts().0 else {
            continue;
        };
        let Some(offset) = original.find(&quote) else {
            continue;
        };
        if evidence.body.match_indices(&quote).nth(1).is_some() {
            continue;
        }
        let quote_text: String = quote.chars().take(EXCERPT_CHARS).collect();
        let (excerpt, clipped) = speaker_excerpt(original, offset, &quote_text);
        citations.push(RecallCitation {
            verified_sender: sender.clone(),
            completed_at_ms: evidence.completed_at_ms,
            speaker_excerpt: (excerpt != quote_text).then_some(excerpt),
            excerpt_truncated: clipped || quote_text.len() < quote.len(),
            quote: quote_text,
        });
    }
    Ok(citations)
}

fn speaker_excerpt(body: &str, offset: usize, quote: &str) -> (String, bool) {
    let spare = EXCERPT_CHARS.saturating_sub(quote.chars().count());
    let end = offset + quote.len();
    let after = body[end..].chars().take(spare.div_ceil(2)).count();
    let before = body[..offset]
        .char_indices()
        .rev()
        .take(spare - after)
        .last()
        .map_or(offset, |(index, _)| index);
    let excerpt: String = body[before..].chars().take(EXCERPT_CHARS).collect();
    let clipped = before > 0 || excerpt.len() < body.len();
    (excerpt, clipped)
}
