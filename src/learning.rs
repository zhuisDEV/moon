//! Durable, bounded L2 reconciliation. The host runs the model; this module owns
//! evidence selection, leases and atomic validation of the resulting changes.
use crate::chunking::sha256_hex;
use crate::memory::{
    distill_in_transaction, fuzzy_context_score, insert_citation, load_evidence, locate_citation,
    reference_terms, sync_memory_indexes,
};
use crate::model::DistillInput;
use crate::redaction::redact_text;
use crate::store::{Store, now_ms, random_nonce};
use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone)]
pub struct PrepareRequest {
    pub run_key: String,
    pub rule_version: String,
    pub review_session: Option<String>,
    pub occurrence: Option<String>,
    pub limit: usize,
    pub max_chars: usize,
    pub lease_ms: i64,
    pub max_attempts: usize,
    pub before_ms: Option<i64>,
    pub after_ms: Option<i64>,
    pub preview: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningEvidence {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_sender: Option<String>,
    pub session_id: String,
    pub document_id: i64,
    pub completed_at_ms: i64,
    pub content: String,
    /// Only selected evidence advances the processing ledger. Supporting older
    /// evidence is included so the model can inspect the original context.
    pub selected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceQuote {
    pub session_id: String,
    pub quote: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningMemory {
    pub document_id: i64,
    pub canonical_key: String,
    pub kind: String,
    pub title: Option<String>,
    pub content: String,
    pub importance: f64,
    pub confidence: f64,
    pub observed_at_ms: Option<i64>,
    pub last_confirmed_at_ms: Option<i64>,
    pub valid_until_ms: Option<i64>,
    pub review_required: bool,
    pub citations: Vec<EvidenceQuote>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplyInput {
    pub actions: Vec<LearningAction>,
    #[serde(default)]
    pub deferred_evidence: Vec<String>,
    #[serde(default)]
    pub metadata: Option<LearningMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LearningMetadata {
    pub model: String,
    pub reasoning: String,
    pub prompt_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<ModelExecution>,
}

/// Requested settings and host observations are deliberately separate. Missing
/// host metadata is unknown; it is never filled from the request as evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelExecution {
    pub requested: RequestedModel,
    pub observed: ObservedModel,
    pub identity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestedModel {
    pub provider: String,
    pub model: String,
    pub reasoning: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservedModel {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub backend: Option<String>,
    pub reasoning: Option<String>,
    pub fallback_attempts: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LearningAction {
    pub action: String,
    pub canonical_key: String,
    pub kind: String,
    #[serde(default)]
    pub title: Option<String>,
    pub content: String,
    pub importance: f64,
    pub confidence: f64,
    #[serde(default)]
    pub target_document_id: Option<i64>,
    #[serde(default)]
    pub merge_document_ids: Vec<i64>,
    pub evidence: Vec<EvidenceQuote>,
    #[serde(default)]
    pub valid_until_ms: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Snapshot {
    #[serde(default)]
    occurrence: String,
    #[serde(default)]
    rule_version: String,
    // Sources captured during a model call were absent at prepare. Match those
    // arrivals against newly indexed deferrals before committing the result.
    #[serde(default)]
    evidence_high_water: Option<i64>,
    // Only consume a wake-up that was actually present in the model packet.
    #[serde(default)]
    retry_support: BTreeMap<String, i64>,
    scope_hash: String,
    selected_sessions: Vec<String>,
    evidence_sessions: Vec<String>,
    memory_ids: Vec<i64>,
}

struct Candidate {
    scope: String,
    session: String,
    chars: usize,
    support: Option<RetrySupport>,
}

struct RetrySupport {
    id: i64,
    session: String,
    chars: usize,
}

impl Store {
    pub fn learning_prepare(&mut self, request: &PrepareRequest) -> Result<Value> {
        validate_identifier(&request.run_key, "run key", 128)?;
        let occurrence = request.occurrence.as_deref().unwrap_or(&request.run_key);
        validate_identifier(occurrence, "learning occurrence", 128)?;
        if !request.rule_version.is_empty() {
            validate_identifier(&request.rule_version, "rule version", 128)?;
        }
        if let Some(session) = &request.review_session {
            crate::memory::validate_session_id(session)?;
        }
        if !(1..=128).contains(&request.limit) || !(1024..=2_097_152).contains(&request.max_chars) {
            anyhow::bail!(
                "learning limits must be 1..128 evidence records and 1024..2097152 characters"
            );
        }
        if !(1_000..=86_400_000).contains(&request.lease_ms)
            || request.before_ms.is_some_and(|v| v <= 0)
            || request.after_ms.is_some_and(|v| v <= 0)
            || matches!((request.after_ms,request.before_ms),(Some(after),Some(before)) if after>before)
            || !(1..=10).contains(&request.max_attempts)
        {
            anyhow::bail!(
                "learning lease must be 1000..86400000 milliseconds, after_ms/before_ms positive and ordered, and max_attempts 1..10"
            );
        }
        let transaction = self
            .connection
            .transaction_with_behavior(if request.preview {
                TransactionBehavior::Deferred
            } else {
                TransactionBehavior::Immediate
            })?;
        if !request.preview {
            if let Some((run_id, deferred_evidence)) = transaction.query_row(
                "SELECT run_id,coalesce(json_extract(outcome_json,'$.deferred_evidence'),0)
                 FROM learning_runs WHERE run_key = ?1 AND status = 'committed' ORDER BY created_at_ms DESC LIMIT 1",
                [&request.run_key], |row| Ok((row.get::<_, String>(0)?,row.get::<_, i64>(1)?)),
            ).optional()? {
                return Ok(json!({"status":"committed", "run_key": request.run_key, "run_id":run_id,"deferred_evidence":deferred_evidence}));
            }
            if transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM learning_runs WHERE status = 'prepared' AND run_id NOT IN (SELECT run_id FROM privacy_legacy_runs) AND lease_until_ms > ?1)",
                [now_ms()], |row| row.get::<_, bool>(0),
            )? {
                return Ok(json!({"status":"busy", "run_key":request.run_key}));
            }
            transaction.execute(
                "UPDATE learning_runs SET status = 'failed', lease_until_ms = NULL, completed_at_ms = ?1 WHERE status = 'prepared' AND run_id NOT IN (SELECT run_id FROM privacy_legacy_runs) AND lease_until_ms <= ?1",
                [now_ms()],
            )?;
            let failed: i64 = transaction.query_row(
                "SELECT count(*) FROM learning_runs WHERE run_key=?1 AND status='failed'",
                [&request.run_key],
                |row| row.get(0),
            )?;
            if failed >= request.max_attempts as i64 {
                transaction.commit()?;
                return Ok(
                    json!({"status":"exhausted","run_key":request.run_key,"failed_attempts":failed}),
                );
            }
        }
        // Old runs had no rule fingerprint. Bind it on the first real versioned
        // prepare, without making every legacy deferral eligible on upgrade.
        if !request.preview && !request.rule_version.is_empty() {
            transaction.execute(
                "UPDATE learning_evidence_retries SET rule_version=?1 WHERE rule_version=''",
                [&request.rule_version],
            )?;
        }
        let mut statement = transaction.prepare(
            "WITH eligible AS (
                SELECT e.session_id, d.scope, e.document_id, e.completed_at_ms, e.id,
                       r.attempts IS NOT NULL AS retry, support.session_id AS support_session,
                       support.id AS support_id, sd.id AS support_document_id
                FROM evidence_sessions e JOIN documents d ON d.id=e.document_id
                LEFT JOIN learning_processed_evidence p ON p.evidence_session_id=e.id
                LEFT JOIN learning_evidence_retries r ON r.evidence_session_id=e.id
                LEFT JOIN evidence_sessions support ON support.id=r.support_id
                    AND NOT EXISTS(SELECT 1 FROM privacy_legacy_documents l WHERE l.document_id=support.document_id)
                    AND (?1 IS NULL OR support.completed_at_ms<=?1)
                LEFT JOIN documents sd ON sd.id=support.document_id AND sd.active=1 AND sd.scope=d.scope
                WHERE p.evidence_session_id IS NULL AND d.active=1
                  AND NOT EXISTS(SELECT 1 FROM historic_speaker_scopes h WHERE h.scope=d.scope)
                  AND NOT EXISTS(SELECT 1 FROM privacy_legacy_documents l WHERE l.document_id=d.id)
                  AND (?1 IS NULL OR e.completed_at_ms<=?1) AND (?2 IS NULL OR e.completed_at_ms>=?2)
                  AND (?3 IS NULL OR (r.attempts IS NOT NULL AND e.session_id=?3))
                  AND (?3 IS NOT NULL OR r.attempts IS NULL OR
                       (r.attempts=1 AND r.last_occurrence<>?4) OR
                       (r.rule_version<>'' AND ?5<>'' AND r.rule_version<>?5) OR sd.id IS NOT NULL)
            ), batch AS MATERIALIZED (
                SELECT * FROM eligible WHERE scope=(SELECT scope FROM eligible ORDER BY retry,completed_at_ms,id LIMIT 1)
                ORDER BY retry,completed_at_ms,id LIMIT ?6
            )
            SELECT b.scope, b.session_id, length(d.body), b.support_session, b.support_id, length(sd.body)
            FROM batch b JOIN documents d ON d.id=b.document_id
            LEFT JOIN documents sd ON sd.id=b.support_document_id ORDER BY b.retry,b.completed_at_ms,b.id",
        )?;
        let candidates = statement
            .query_map(
                params![
                    request.before_ms,
                    request.after_ms,
                    request.review_session,
                    occurrence,
                    request.rule_version,
                    request.limit as i64
                ],
                |row| {
                    let support = match row.get::<_, Option<i64>>(5)? {
                        Some(chars) => Some(RetrySupport {
                            session: row.get(3)?,
                            id: row.get(4)?,
                            chars: chars as usize,
                        }),
                        None => None,
                    };
                    Ok(Candidate {
                        scope: row.get(0)?,
                        session: row.get(1)?,
                        chars: row.get::<_, i64>(2)? as usize,
                        support,
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        let Some(scope) = candidates.first().map(|item| item.scope.clone()) else {
            transaction.commit()?;
            return Ok(json!({"status":"empty", "run_key":request.run_key}));
        };
        let mut evidence = Vec::new();
        let mut memories = Vec::new();
        // Measure the complete envelope as well as data. A long scope or run
        // key must not silently push a supposedly bounded packet over budget.
        let envelope_chars = serde_json::to_string(&json!({
            "status":"prepared","run_id":"0".repeat(32),"run_key":request.run_key,
            "scope":scope,"snapshot_hash":"0".repeat(64),"evidence":[],"memories":[],
            "context_limited":false,"omitted_memory_count":128
        }))?
        .chars()
        .count()
        .saturating_sub(input_chars(&[], &[])?);
        let budget = request.max_chars.saturating_sub(envelope_chars);
        let mut retry_support = BTreeMap::new();
        for candidate in candidates {
            let session = candidate.session;
            // Keep both sides of a wake-up inside the same bounded packet. The
            // supporting turn may already have been processed, or selected here.
            let missing_support = candidate.support.as_ref().filter(|support| {
                !evidence
                    .iter()
                    .any(|e: &LearningEvidence| e.session_id == support.session)
            });
            if candidate.chars > budget || missing_support.is_some_and(|item| item.chars > budget) {
                if evidence.is_empty() {
                    anyhow::bail!(
                        "oldest pending evidence exceeds max_chars; increase the learning input budget or review that evidence explicitly"
                    );
                }
                break;
            }
            let previous_len = evidence.len();
            let existing = evidence.iter().position(|e| e.session_id == session);
            if let Some(index) = existing {
                evidence[index].selected = true;
            } else {
                let mut item = read_learning_evidence(&transaction, &session)?;
                item.selected = true;
                evidence.push(item);
            }
            if let Some(support) = missing_support {
                evidence.push(read_learning_evidence(&transaction, &support.session)?);
            }
            let used = input_chars(&evidence, &memories)?;
            // Reserve half for prior claims and their evidence except for the
            // first complete selection (including any required supporting turn).
            if used > budget || (previous_len > 0 && used > budget / 2) {
                evidence.truncate(previous_len);
                if let Some(index) = existing {
                    evidence[index].selected = false;
                }
                if evidence.is_empty() {
                    anyhow::bail!(
                        "oldest pending evidence exceeds max_chars; increase the learning input budget or review that evidence explicitly"
                    );
                }
                break;
            }
            if let Some(support) = candidate.support {
                retry_support.insert(session, support.id);
            }
        }
        let query = evidence
            .iter()
            .map(|e| e.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let mut omitted_memory_count = 0;
        for memory in related_memories(&transaction, &scope, &query, 128)? {
            let previous_len = evidence.len();
            for citation in &memory.citations {
                if !evidence.iter().any(|e| e.session_id == citation.session_id) {
                    let citation_scope:String=transaction.query_row("SELECT d.scope FROM evidence_sessions e JOIN documents d ON d.id=e.document_id WHERE e.session_id=?1",[&citation.session_id],|row|row.get(0))?;
                    if citation_scope != scope {
                        anyhow::bail!(
                            "stored memory citation crosses learning scopes; review its evidence before reconciliation"
                        );
                    }
                    let item = read_learning_evidence(&transaction, &citation.session_id)?;
                    evidence.push(item);
                }
            }
            memories.push(memory);
            if input_chars(&evidence, &memories)? > budget {
                evidence.truncate(previous_len);
                memories.pop();
                omitted_memory_count += 1;
            }
        }
        if memories.is_empty() && omitted_memory_count > 0 {
            anyhow::bail!(
                "related memory and original evidence cannot fit max_chars; increase the input budget before reconciliation"
            );
        }
        let evidence_high_water: i64 = transaction.query_row(
            "SELECT coalesce(max(id),0) FROM evidence_sessions",
            [],
            |row| row.get(0),
        )?;
        let snapshot = Snapshot {
            evidence_high_water: Some(evidence_high_water),
            occurrence: occurrence.to_owned(),
            rule_version: request.rule_version.clone(),
            retry_support,
            scope_hash: scope_hash(&transaction, &scope)?,
            selected_sessions: evidence
                .iter()
                .filter(|e| e.selected)
                .map(|e| e.session_id.clone())
                .collect(),
            evidence_sessions: evidence.iter().map(|e| e.session_id.clone()).collect(),
            memory_ids: memories.iter().map(|m| m.document_id).collect(),
        };
        let snapshot_json = serde_json::to_string(&snapshot)?;
        let snapshot_hash = sha256_hex(&snapshot_json);
        let run_id = if request.preview {
            None
        } else {
            let run_id = random_nonce(&transaction)?;
            transaction.execute(
                "INSERT INTO learning_runs(run_id,run_key,scope,status,created_at_ms,lease_until_ms,snapshot_hash,snapshot_json)
                 VALUES(?1,?2,?3,'prepared',?4,?5,?6,?7)",
                params![run_id, request.run_key, scope, now_ms(), now_ms() + request.lease_ms, snapshot_hash, snapshot_json],
            )?;
            Some(run_id)
        };
        transaction.commit()?;
        Ok(
            json!({"status":if request.preview {"preview"} else {"prepared"}, "run_id":run_id,
            "run_key":request.run_key,"scope":scope,"snapshot_hash":snapshot_hash,
            "evidence":evidence,"memories":memories,"context_limited":omitted_memory_count>0,"omitted_memory_count":omitted_memory_count}),
        )
    }

    pub fn learning_related(
        &self,
        scope: &str,
        query: &str,
        limit: usize,
        max_chars: usize,
    ) -> Result<Value> {
        if scope.is_empty()
            || query.is_empty()
            || query.len() > 64_000
            || !(1..=128).contains(&limit)
            || !(1024..=128_000).contains(&max_chars)
        {
            anyhow::bail!("invalid related-memory scope, query or limits");
        }
        let mut memories = Vec::new();
        for memory in related_memories(&self.connection, scope, query, limit)? {
            memories.push(memory);
            if serde_json::to_string(&memories)?.chars().count() > max_chars.saturating_sub(64) {
                memories.pop();
            }
        }
        Ok(json!({"memories":memories}))
    }

    pub fn learning_apply(
        &mut self,
        run_id: &str,
        input: ApplyInput,
        dry_run: bool,
    ) -> Result<Value> {
        validate_identifier(run_id, "run id", 128)?;
        if input.actions.len() > 32 {
            anyhow::bail!("learning apply accepts at most 32 actions");
        }
        if input.deferred_evidence.len() > 128 {
            anyhow::bail!("learning apply accepts at most 128 deferred evidence sessions");
        }
        if let Some(metadata) = &input.metadata {
            validate_metadata(metadata)?;
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (status, scope, lease, snapshot_json, outcome) = transaction.query_row(
            "SELECT status,scope,lease_until_ms,snapshot_json,outcome_json FROM learning_runs WHERE run_id = ?1 AND run_id NOT IN (SELECT run_id FROM privacy_legacy_runs)",
            [run_id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, Option<i64>>(2)?, row.get::<_, String>(3)?, row.get::<_, Option<String>>(4)?)),
        ).optional()?.context("learning run does not exist")?;
        if status == "committed" {
            return serde_json::from_str(
                &outcome.context("committed learning outcome is missing")?,
            )
            .context("invalid stored learning outcome");
        }
        if status != "prepared" || lease.is_none_or(|until| until <= now_ms()) {
            anyhow::bail!("learning run lease is no longer active; prepare a fresh run");
        }
        let snapshot: Snapshot = serde_json::from_str(&snapshot_json)?;
        let mut deferred_sessions = input.deferred_evidence;
        let mut deferred = BTreeSet::new();
        for session in &deferred_sessions {
            if !deferred.insert(session.clone()) {
                anyhow::bail!("deferred evidence sessions must be unique");
            }
            if !snapshot.selected_sessions.contains(session) {
                anyhow::bail!("deferred evidence must belong to the selected prepared sessions");
            }
        }
        if snapshot.scope_hash != scope_hash(&transaction, &scope)? {
            anyhow::bail!(
                "memory changed after learning prepare; fail this run and prepare a fresh snapshot"
            );
        }
        let mut outcomes = Vec::new();
        let mut touched = BTreeSet::new();
        for action in &input.actions {
            validate_action(&transaction, &scope, &snapshot, action)?;
            if !touched.insert(action.canonical_key.clone()) {
                anyhow::bail!("a learning batch may change each canonical key only once");
            }
            outcomes.push(apply_action(&transaction, run_id, &scope, action)?);
        }
        let cited: BTreeSet<_> = input
            .actions
            .iter()
            .filter(|action| action.action != "review")
            .flat_map(|action| {
                action
                    .evidence
                    .iter()
                    .map(|citation| citation.session_id.as_str())
            })
            .collect();
        // An empty or unrelated successful batch can close ordinary fresh
        // turns. Previously rejected evidence needs an accepted, cited claim.
        let mut newly_deferred = false;
        for session in &snapshot.selected_sessions {
            let (id, prior_attempts): (i64, Option<u8>) = transaction.query_row(
                "SELECT e.id,r.attempts FROM evidence_sessions e LEFT JOIN learning_evidence_retries r
                 ON r.evidence_session_id=e.id WHERE e.session_id=?1", [session], |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            if prior_attempts.is_some()
                && !cited.contains(session.as_str())
                && deferred.insert(session.clone())
            {
                deferred_sessions.push(session.clone());
            }
            if deferred.contains(session) {
                let occurrence = if snapshot.occurrence.is_empty() {
                    run_id
                } else {
                    &snapshot.occurrence
                };
                // A concurrent capture may have supplied a newer trigger since
                // prepare. Consume only the support included in this snapshot.
                transaction.execute(
                    "INSERT INTO learning_evidence_retries(evidence_session_id,attempts,last_occurrence,rule_version,support_after_id)
                     VALUES(?1,1,?2,?3,coalesce(?5,(SELECT max(id) FROM evidence_sessions))) ON CONFLICT(evidence_session_id) DO UPDATE SET
                     attempts=2,last_occurrence=excluded.last_occurrence,
                     rule_version=CASE WHEN excluded.rule_version='' THEN rule_version ELSE excluded.rule_version END,
                     support_after_id=max(support_after_id,coalesce(?4,0)),
                     support_id=CASE WHEN support_id=?4 THEN NULL ELSE support_id END",
                    params![id,occurrence,snapshot.rule_version,snapshot.retry_support.get(session),snapshot.evidence_high_water],
                )?;
                if prior_attempts.is_none() {
                    index_retry_terms(&transaction, id)?;
                    newly_deferred = true;
                }
            } else {
                transaction.execute(
                    "DELETE FROM learning_evidence_retries WHERE evidence_session_id=?1",
                    [id],
                )?;
                transaction.execute(
                    "INSERT INTO learning_processed_evidence(evidence_session_id,run_id,processed_at_ms)
                     VALUES(?1,?2,?3)", params![id,run_id,now_ms()],
                )?;
            }
        }
        if newly_deferred && let Some(high_water) = snapshot.evidence_high_water {
            match_arrivals_since(&transaction, high_water, Some(&scope))?;
        }
        let outcome = json!({"status":if dry_run {"dry_run"} else {"committed"},"run_id":run_id,
            "action_count":outcomes.len(), "processed_evidence":if dry_run {0} else {snapshot.selected_sessions.len()-deferred.len()},
            "selected_evidence":snapshot.selected_sessions.len(), "outcomes":outcomes,
            "deferred_evidence":deferred.len(),"deferred_session_ids":deferred_sessions,
            "metadata":input.metadata});
        if dry_run {
            transaction.rollback()?;
        } else {
            transaction.execute(
                "UPDATE learning_runs SET status='committed',lease_until_ms=NULL,completed_at_ms=?2,outcome_json=?3 WHERE run_id=?1",
                params![run_id,now_ms(),serde_json::to_string(&outcome)?],
            )?;
            transaction.commit()?;
        }
        Ok(outcome)
    }

    pub fn learning_fail(&mut self, run_id: &str) -> Result<Value> {
        validate_identifier(run_id, "run id", 128)?;
        let changed = self.connection.execute(
            "UPDATE learning_runs SET status='failed',lease_until_ms=NULL,completed_at_ms=?2 WHERE run_id=?1 AND status='prepared'",
            params![run_id,now_ms()],
        )?;
        // Never accept an arbitrary provider error body for persistence.
        let status: String = self
            .connection
            .query_row(
                "SELECT status FROM learning_runs WHERE run_id=?1",
                [run_id],
                |row| row.get(0),
            )
            .optional()?
            .context("learning run does not exist")?;
        Ok(json!({"status":status,"run_id":run_id,"changed":changed == 1}))
    }

    pub fn learning_deferred(&self, after_id: i64, limit: usize) -> Result<Value> {
        anyhow::ensure!(
            after_id >= 0 && (1..=128).contains(&limit),
            "after-id must be nonnegative and limit must be between 1 and 128"
        );
        let mut statement = self.connection.prepare(
            "SELECT e.id,e.session_id,d.scope,r.attempts
             FROM learning_evidence_retries r JOIN evidence_sessions e ON e.id=r.evidence_session_id
             JOIN documents d ON d.id=e.document_id
             WHERE r.evidence_session_id>?1 AND d.active=1
               AND NOT EXISTS(SELECT 1 FROM privacy_legacy_documents l WHERE l.document_id=d.id)
             ORDER BY r.evidence_session_id LIMIT ?2",
        )?;
        let mut rows = statement.query_map(params![after_id,(limit+1) as i64], |row| {
            let attempts: i64 = row.get(3)?;
            Ok(json!({"id":row.get::<_,i64>(0)?, "session_id":row.get::<_,String>(1)?,
                "scope":row.get::<_,String>(2)?, "state":if attempts == 1 {"retry_pending"} else {"parked"}}))
        })?.collect::<rusqlite::Result<Vec<_>>>()?;
        let next_after_id = if rows.len() > limit {
            rows.truncate(limit);
            rows.last().and_then(|row| row["id"].as_i64())
        } else {
            None
        };
        Ok(json!({"deferred":rows, "next_after_id":next_after_id}))
    }

    pub fn learning_status(&self) -> Result<Value> {
        let mut statement = self.connection.prepare(
            "SELECT run_id,run_key,scope,status,created_at_ms,completed_at_ms,lease_until_ms,
                    coalesce(json_extract(outcome_json,'$.deferred_evidence'),0)
             FROM learning_runs ORDER BY created_at_ms DESC,rowid DESC LIMIT 20",
        )?;
        let runs = statement.query_map([], |row| Ok(json!({
            "run_id":row.get::<_,String>(0)?,"run_key":row.get::<_,String>(1)?,"scope":row.get::<_,String>(2)?,
            "status":row.get::<_,String>(3)?,"created_at_ms":row.get::<_,i64>(4)?,"completed_at_ms":row.get::<_,Option<i64>>(5)?,"lease_until_ms":row.get::<_,Option<i64>>(6)?,
            "deferred_evidence":row.get::<_,i64>(7)?
        })))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let pending: i64 = self.connection.query_row("SELECT count(*) FROM evidence_sessions e JOIN documents d ON d.id=e.document_id LEFT JOIN learning_processed_evidence p ON p.evidence_session_id=e.id WHERE p.evidence_session_id IS NULL AND NOT EXISTS(SELECT 1 FROM privacy_legacy_documents l WHERE l.document_id=e.document_id) AND NOT EXISTS(SELECT 1 FROM historic_speaker_scopes h WHERE h.scope=d.scope)", [], |row| row.get(0))?;
        let (retry_pending, parked): (i64, i64) = self.connection.query_row(
            "SELECT count(*) FILTER (WHERE attempts=1),count(*) FILTER (WHERE attempts=2)
             FROM learning_evidence_retries r JOIN evidence_sessions e ON e.id=r.evidence_session_id JOIN documents d ON d.id=e.document_id
             WHERE NOT EXISTS(SELECT 1 FROM privacy_legacy_documents l WHERE l.document_id=e.document_id) AND NOT EXISTS(SELECT 1 FROM historic_speaker_scopes h WHERE h.scope=d.scope)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let processed: i64 = self.connection.query_row(
            "SELECT count(*) FROM learning_processed_evidence",
            [],
            |row| row.get(0),
        )?;
        let mut statement = self.connection.prepare("SELECT id,run_id,document_id,scope,canonical_key,created_at_ms,reason,evidence_ids_json FROM learning_reviews ORDER BY id DESC LIMIT 100")?;
        let reviews = statement.query_map([], |row| Ok(json!({"review_id":row.get::<_,i64>(0)?,"run_id":row.get::<_,String>(1)?,"document_id":row.get::<_,Option<i64>>(2)?,"scope":row.get::<_,String>(3)?,"canonical_key":row.get::<_,String>(4)?,"created_at_ms":row.get::<_,i64>(5)?,"reason":row.get::<_,String>(6)?,"evidence_session_ids":serde_json::from_str::<Value>(&row.get::<_,String>(7)?).unwrap_or(Value::Null)})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(
            json!({"pending_evidence":pending,"retry_pending_evidence":retry_pending,"parked_evidence":parked,"processed_evidence":processed,"runs":runs,"reviews":reviews}),
        )
    }
}

// Use Moon's existing multilingual reference tokenisation, excluding role
// labels and generic conversation words. Two distinct terms are the minimum
// trigger; longer turns need additional overlap. The model still has to ground
// any eventual claim in the original evidence.
fn retry_terms(content: &str) -> Vec<String> {
    reference_terms(content)
        .into_iter()
        .filter(|term| {
            term.len() <= 64
                && !term.chars().all(char::is_numeric)
                && !matches!(
                    term.as_str(),
                    "user"
                        | "assistant"
                        | "system"
                        | "tool"
                        | "message"
                        | "thank"
                        | "have"
                        | "will"
                        | "want"
                        | "need"
                        | "yes"
                )
        })
        .collect()
}

fn index_retry_terms(connection: &Connection, id: i64) -> Result<()> {
    let (scope, body): (String,String) = connection.query_row(
        "SELECT d.scope,d.body FROM evidence_sessions e JOIN documents d ON d.id=e.document_id WHERE e.id=?1",
        [id], |row| Ok((row.get(0)?,row.get(1)?)),
    )?;
    let terms = retry_terms(&body);
    connection.execute(
        "INSERT INTO learning_retry_terms(scope,term,evidence_session_id)
         SELECT ?1,value,?2 FROM json_each(?3)",
        params![scope, id, serde_json::to_string(&terms)?],
    )?;
    Ok(())
}

pub(crate) fn index_migrated_deferrals(connection: &Connection) -> Result<()> {
    let mut statement =
        connection.prepare("SELECT evidence_session_id FROM learning_evidence_retries")?;
    let rows = statement.query_map([], |row| row.get::<_, i64>(0))?;
    for id in rows {
        index_retry_terms(connection, id?)?;
    }
    let oldest_watermark: Option<i64> = connection.query_row(
        "SELECT min(support_after_id) FROM learning_evidence_retries",
        [],
        |row| row.get(0),
    )?;
    if let Some(id) = oldest_watermark {
        match_arrivals_since(connection, id, None)?;
    }
    Ok(())
}

fn match_arrivals_since(connection: &Connection, after: i64, scope: Option<&str>) -> Result<()> {
    let mut statement = connection.prepare(
        "SELECT e.id,d.scope FROM evidence_sessions e JOIN documents d ON d.id=e.document_id
         WHERE e.id>?1 AND d.active=1 AND (?2 IS NULL OR d.scope=?2) ORDER BY e.id",
    )?;
    let arrivals = statement.query_map(params![after, scope], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    for arrival in arrivals {
        let (new_id, scope) = arrival?;
        match_new_retry_support(connection, new_id, &scope)?;
    }
    Ok(())
}

pub(crate) fn match_new_retry_support(
    connection: &Connection,
    new_id: i64,
    scope: &str,
) -> Result<()> {
    if !connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM learning_retry_terms WHERE scope=?1)",
        [scope],
        |row| row.get::<_, bool>(0),
    )? {
        return Ok(());
    }
    let body: String = connection.query_row(
        "SELECT d.body FROM evidence_sessions e JOIN documents d ON d.id=e.document_id WHERE e.id=?1",
        [new_id], |row| row.get(0),
    )?;
    let terms = retry_terms(&body);
    connection.execute(
        "UPDATE learning_evidence_retries SET support_id=?1
         WHERE support_after_id < ?1 AND coalesce(support_id,0) < ?1 AND evidence_session_id <> ?1
           AND evidence_session_id IN (
            SELECT t.evidence_session_id FROM json_each(?3) j
            JOIN learning_retry_terms t ON t.scope=?2 AND t.term=j.value
            GROUP BY t.evidence_session_id
            HAVING count(*) >= max(2, (min(12, ?4, (
                SELECT count(*) FROM learning_retry_terms total WHERE total.evidence_session_id=t.evidence_session_id
            )) + 2) / 3)
        )", params![new_id,scope,serde_json::to_string(&terms)?,terms.len() as i64],
    )?;
    Ok(())
}

fn input_chars(evidence: &[LearningEvidence], memories: &[LearningMemory]) -> Result<usize> {
    Ok(
        serde_json::to_string(&json!({"evidence":evidence,"memories":memories}))?
            .chars()
            .count(),
    )
}

fn read_learning_evidence(connection: &Connection, session: &str) -> Result<LearningEvidence> {
    Ok(connection.query_row(
        "SELECT e.session_id,e.document_id,e.completed_at_ms,d.body,t.sender FROM evidence_sessions e JOIN documents d ON d.id=e.document_id
         LEFT JOIN evidence_admissions b ON b.evidence_id=e.id LEFT JOIN privacy_admissions t ON t.admission_key=b.admission_key WHERE e.session_id=?1",
        [session], |row| Ok(LearningEvidence {verified_sender:row.get(4)?,session_id:row.get(0)?, document_id:row.get(1)?, completed_at_ms:row.get(2)?, content:row.get(3)?,selected:false}),
    )?)
}

fn related_memories(
    connection: &Connection,
    scope: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<LearningMemory>> {
    let mut statement = connection.prepare(
        "SELECT m.document_id,m.canonical_key,m.memory_kind,d.title,d.body,m.importance,m.confidence,m.observed_at_ms,m.last_confirmed_at_ms,m.valid_until_ms,
                EXISTS(SELECT 1 FROM learning_reviews r WHERE r.document_id=d.id OR (r.document_id IS NULL AND r.scope=d.scope AND r.canonical_key=m.canonical_key))
         FROM memory_items m JOIN documents d ON d.id=m.document_id WHERE d.scope=?1 AND d.active=1
           AND NOT EXISTS(SELECT 1 FROM privacy_legacy_documents l WHERE l.document_id=d.id)
           AND (NOT EXISTS (SELECT 1 FROM memory_audiences WHERE scope=?1)
                OR d.id IN (SELECT document_id FROM audience_recallable_memories WHERE scope=?1))
           AND m.superseded_by IS NULL AND m.canonical_key IS NOT NULL AND length(d.body)<=32000
         ORDER BY m.last_confirmed_at_ms DESC,m.document_id DESC LIMIT 512",
    )?;
    let mut rows = statement
        .query_map([scope], |row| {
            Ok(LearningMemory {
                document_id: row.get(0)?,
                canonical_key: row.get(1)?,
                kind: row.get(2)?,
                title: row.get(3)?,
                content: row.get(4)?,
                importance: row.get(5)?,
                confidence: row.get(6)?,
                observed_at_ms: row.get(7)?,
                last_confirmed_at_ms: row.get(8)?,
                valid_until_ms: row.get(9)?,
                review_required: row.get(10)?,
                citations: Vec::new(),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .filter_map(|memory| {
            // A memory's short terms are the query here: a long batch should not
            // dilute an entity match merely because it covers several topics.
            let score = fuzzy_context_score(
                &format!("{} {}", memory.canonical_key, memory.content),
                query,
            )
            .or_else(|| fuzzy_context_score(query, &memory.content));
            score.map(|score| (score, memory))
        })
        .collect::<Vec<_>>();
    rows.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then_with(|| b.1.last_confirmed_at_ms.cmp(&a.1.last_confirmed_at_ms))
    });
    let mut result = Vec::new();
    for (_, mut memory) in rows.into_iter().take(limit) {
        let mut citations = connection.prepare(
            "SELECT e.session_id,c.quote FROM memory_citations c JOIN evidence_sessions e ON e.id=c.evidence_session_id
             JOIN documents evidence ON evidence.id=e.document_id
             WHERE c.memory_document_id=?1 AND evidence.scope=?2 AND c.evidence_document_id=e.document_id
             ORDER BY e.completed_at_ms DESC,c.id DESC LIMIT 8",
        )?;
        memory.citations = citations
            .query_map(params![memory.document_id, scope], |row| {
                Ok(EvidenceQuote {
                    session_id: row.get(0)?,
                    quote: row.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        result.push(memory);
    }
    Ok(result)
}

fn scope_hash(connection: &Connection, scope: &str) -> Result<String> {
    let mut statement = connection.prepare(
        "SELECT m.document_id,d.content_hash,m.canonical_key,m.memory_kind,m.last_confirmed_at_ms,m.valid_until_ms,m.importance,m.confidence,m.pinned,
                (SELECT count(*) FROM memory_citations c WHERE c.memory_document_id=m.document_id)
         FROM memory_items m JOIN documents d ON d.id=m.document_id WHERE d.scope=?1 AND d.active=1 AND m.superseded_by IS NULL ORDER BY m.document_id",
    )?;
    let rows = statement
        .query_map([scope], |row| {
            Ok(json!([
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, f64>(6)?,
                row.get::<_, f64>(7)?,
                row.get::<_, bool>(8)?,
                row.get::<_, i64>(9)?
            ]))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut aliases = connection.prepare("SELECT a.canonical_key,a.document_id FROM memory_aliases a JOIN documents d ON d.id=a.document_id WHERE d.scope=?1 ORDER BY a.canonical_key")?;
    let aliases = aliases
        .query_map([scope], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(sha256_hex(&serde_json::to_string(&(rows, aliases))?))
}

fn validate_action(
    transaction: &Transaction<'_>,
    scope: &str,
    snapshot: &Snapshot,
    action: &LearningAction,
) -> Result<()> {
    validate_identifier(&action.canonical_key, "canonical key", 256)?;
    validate_identifier(&action.kind, "memory kind", 128)?;
    if !matches!(
        action.action.as_str(),
        "create" | "confirm" | "supersede" | "merge" | "review"
    ) {
        anyhow::bail!("unsupported learning action");
    }
    if action.content.trim().is_empty()
        || action.content.len() > 16_384
        || !(0.0..=1.0).contains(&action.importance)
        || !(0.0..=1.0).contains(&action.confidence)
    {
        anyhow::bail!("invalid learning content, importance or confidence");
    }
    if action.evidence.is_empty()
        || action.evidence.len() > 16
        || action.merge_document_ids.len() > 32
    {
        anyhow::bail!("learning actions need 1..16 evidence quotes and at most 32 merge targets");
    }
    let mut has_selected = false;
    let mut quotes = Vec::new();
    for citation in &action.evidence {
        if !snapshot.evidence_sessions.contains(&citation.session_id) {
            anyhow::bail!("learning action cites evidence outside its prepared snapshot");
        }
        has_selected |= snapshot.selected_sessions.contains(&citation.session_id);
        if citation.quote.chars().count() < 8 || citation.quote.len() > 8192 {
            anyhow::bail!("learning evidence quotes must be 8 characters through 8192 bytes");
        }
        let evidence = load_evidence(transaction, &citation.session_id)?;
        if evidence.scope != scope {
            anyhow::bail!("learning evidence must remain within its scope");
        }
        locate_citation(&evidence.body, &citation.quote)?;
        quotes.push((evidence.completed_at_ms, citation.quote.as_str()));
    }
    if !has_selected {
        anyhow::bail!(
            "learning actions must cite at least one selected, unprocessed evidence session"
        );
    }
    if action.action != "review" {
        let newest = quotes
            .iter()
            .map(|(date, _)| *date)
            .max()
            .context("missing evidence")?;
        let newest_quotes = quotes
            .iter()
            .filter(|(date, _)| *date == newest)
            .map(|(_, quote)| *quote)
            .collect::<Vec<_>>();
        validate_grounding(&action.content, &newest_quotes)?;
    }
    if action.action == "create"
        && (action.target_document_id.is_some() || !action.merge_document_ids.is_empty())
    {
        anyhow::bail!("create cannot target an existing memory");
    }
    if matches!(action.action.as_str(), "confirm" | "supersede" | "merge")
        && action.target_document_id.is_none()
    {
        anyhow::bail!("this learning action requires target_document_id");
    }
    if action.action != "merge" && !action.merge_document_ids.is_empty() {
        anyhow::bail!("merge_document_ids are valid only for merge");
    }
    for target in action
        .target_document_id
        .iter()
        .chain(action.merge_document_ids.iter())
    {
        if !snapshot.memory_ids.contains(target) {
            anyhow::bail!("learning target is not in the prepared memory snapshot");
        }
        let current = transaction.query_row(
            "SELECT d.scope,m.memory_kind,m.canonical_key,d.body FROM memory_items m JOIN documents d ON d.id=m.document_id WHERE m.document_id=?1 AND d.active=1 AND m.superseded_by IS NULL",
            [target], |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,String>(3)?)),
        ).optional()?.context("learning target is no longer active")?;
        if current.0 != scope || current.1 != action.kind {
            anyhow::bail!("learning targets must share scope and memory kind");
        }
        if Some(target) == action.target_document_id.as_ref() && current.2 != action.canonical_key {
            anyhow::bail!("target canonical key does not match the learning action");
        }
        if matches!(action.action.as_str(), "confirm" | "merge")
            && current.3 != redact_text(&action.content).value
        {
            anyhow::bail!(
                "confirm and merge must preserve exact existing content; use supersede or review for different claims"
            );
        }
    }
    Ok(())
}

fn apply_action(
    transaction: &Transaction<'_>,
    run_id: &str,
    scope: &str,
    action: &LearningAction,
) -> Result<Value> {
    if action.action == "review" {
        let reason = redact_text(&action.content)
            .value
            .chars()
            .take(2000)
            .collect::<String>();
        transaction.execute(
            "INSERT INTO learning_reviews(run_id,document_id,scope,canonical_key,reason_hash,evidence_ids_json,created_at_ms,reason) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![run_id,action.target_document_id,scope,action.canonical_key,sha256_hex(&reason),serde_json::to_string(&action.evidence.iter().map(|e|&e.session_id).collect::<Vec<_>>())?,now_ms(),reason],
        )?;
        return Ok(
            json!({"action":"review","document_id":action.target_document_id,"review_id":transaction.last_insert_rowid()}),
        );
    }
    let newest = action
        .evidence
        .iter()
        .map(|quote| {
            Ok((
                load_evidence(transaction, &quote.session_id)?.completed_at_ms,
                quote,
            ))
        })
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .max_by_key(|(date, _)| *date)
        .context("missing learning evidence")?
        .1;
    let outcome = distill_in_transaction(
        transaction,
        DistillInput {
            canonical_key: action.canonical_key.clone(),
            memory_kind: action.kind.clone(),
            scope: scope.into(),
            title: action.title.clone(),
            content: action.content.clone(),
            importance: action.importance,
            confidence: action.confidence,
            pinned: false,
            evidence_session_id: newest.session_id.clone(),
            evidence_quote: newest.quote.clone(),
            supersedes: if action.action == "supersede" {
                action.target_document_id
            } else {
                None
            },
            valid_until_ms: action.valid_until_ms,
        },
    )?;
    for quote in &action.evidence {
        let evidence = load_evidence(transaction, &quote.session_id)?;
        let location = locate_citation(&evidence.body, &quote.quote)?;
        insert_citation(
            transaction,
            outcome.document_id,
            &evidence,
            &location,
            &quote.quote,
        )?;
    }
    if action.action == "merge" {
        if action.merge_document_ids.is_empty() {
            anyhow::bail!("merge requires at least one other memory");
        }
        let mut unique = BTreeSet::new();
        for target in &action.merge_document_ids {
            if *target == outcome.document_id || !unique.insert(*target) {
                anyhow::bail!(
                    "merge targets must be distinct from each other and the retained memory"
                );
            }
            merge_identical(transaction, *target, outcome.document_id, run_id)?;
        }
    }
    Ok(
        json!({"action":action.action,"document_id":outcome.document_id,"distill_action":outcome.action,"superseded_document_id":outcome.superseded_document_id,"merged_document_ids":action.merge_document_ids}),
    )
}

fn merge_identical(
    transaction: &Transaction<'_>,
    previous: i64,
    retained: i64,
    run_id: &str,
) -> Result<()> {
    let old_key: String = transaction.query_row(
        "SELECT canonical_key FROM memory_items WHERE document_id=?1",
        [previous],
        |row| row.get(0),
    )?;
    // Identical text does not make its confirmation dates, expiry or review
    // status interchangeable. Preserve the newest source confirmation and any
    // existing expiry; a duplicate key must not revive a temporary observation.
    transaction.execute(
        "UPDATE memory_items SET
           valid_until_ms = CASE
             WHEN valid_until_ms IS NULL THEN (SELECT valid_until_ms FROM memory_items WHERE document_id=?1)
             WHEN (SELECT coalesce(last_confirmed_at_ms,0) FROM memory_items WHERE document_id=?1)>coalesce(last_confirmed_at_ms,0)
               THEN coalesce((SELECT valid_until_ms FROM memory_items WHERE document_id=?1),valid_until_ms)
             ELSE valid_until_ms END,
           observed_at_ms = min(coalesce(observed_at_ms,(SELECT observed_at_ms FROM memory_items WHERE document_id=?1)),
               coalesce((SELECT observed_at_ms FROM memory_items WHERE document_id=?1),observed_at_ms)),
           last_confirmed_at_ms = max(coalesce(last_confirmed_at_ms,0),(SELECT coalesce(last_confirmed_at_ms,0) FROM memory_items WHERE document_id=?1)),
           importance = max(importance,(SELECT importance FROM memory_items WHERE document_id=?1)),
           confidence = max(confidence,(SELECT confidence FROM memory_items WHERE document_id=?1)),
           pinned = max(pinned,(SELECT pinned FROM memory_items WHERE document_id=?1))
         WHERE document_id=?2",
        params![previous,retained],
    )?;
    // Keep the original review immutable while carrying the unresolved conflict
    // onto the retained identity. A future supersession gets a distinct head.
    transaction.execute(
        "INSERT INTO learning_reviews(run_id,document_id,scope,canonical_key,reason_hash,reason,evidence_ids_json,created_at_ms)
         SELECT ?3,?2,r.scope,(SELECT canonical_key FROM memory_items WHERE document_id=?2),r.reason_hash,r.reason,r.evidence_ids_json,?4
         FROM learning_reviews r
         WHERE (r.document_id=?1 OR (r.document_id IS NULL AND r.canonical_key=?5
                AND r.scope=(SELECT scope FROM documents WHERE id=?1)))
           AND NOT EXISTS(SELECT 1 FROM learning_reviews retained_review WHERE retained_review.document_id=?2
                AND retained_review.reason_hash=r.reason_hash AND retained_review.evidence_ids_json=r.evidence_ids_json)",
        params![previous,retained,run_id,now_ms(),old_key],
    )?;
    transaction.execute("INSERT OR IGNORE INTO memory_citations(memory_document_id,evidence_session_id,evidence_document_id,start_byte,end_byte,start_line,end_line,quote,created_at_ms) SELECT ?2,evidence_session_id,evidence_document_id,start_byte,end_byte,start_line,end_line,quote,created_at_ms FROM memory_citations WHERE memory_document_id=?1",params![previous,retained])?;
    transaction.execute("DELETE FROM memory_heads WHERE document_id=?1", [previous])?;
    transaction.execute(
        "UPDATE memory_items SET superseded_by=?2,valid_until_ms=CASE
           WHEN valid_until_ms IS NULL THEN ?3 ELSE min(valid_until_ms,?3) END WHERE document_id=?1",
        params![previous, retained, now_ms()],
    )?;
    transaction.execute(
        "INSERT INTO memory_aliases(scope,canonical_key,document_id,created_at_ms)
         SELECT scope,?1,?2,?3 FROM documents WHERE id=?2
         ON CONFLICT(scope,canonical_key) DO UPDATE SET document_id=excluded.document_id",
        params![old_key, retained, now_ms()],
    )?;
    transaction.execute(
        "UPDATE memory_aliases SET document_id=?2 WHERE document_id=?1",
        params![previous, retained],
    )?;
    transaction.execute(
        "DELETE FROM chunk_fts WHERE rowid IN (SELECT id FROM chunks WHERE document_id=?1)",
        [previous],
    )?;
    transaction.execute(
        "DELETE FROM chunk_vectors WHERE rowid IN (SELECT id FROM chunks WHERE document_id=?1)",
        [previous],
    )?;
    transaction.execute("DELETE FROM embedding_queue WHERE chunk_id IN (SELECT id FROM chunks WHERE document_id=?1)",[previous])?;
    transaction.execute(
        "UPDATE chunks SET embedding_model=NULL,embedded_at_ms=NULL WHERE document_id=?1",
        [previous],
    )?;
    sync_memory_indexes(transaction, retained)?;
    Ok(())
}

fn validate_identifier(value: &str, label: &str, max: usize) -> Result<()> {
    if value.is_empty()
        || value.len() > max
        || !value
            .chars()
            .all(|c| c.is_alphanumeric() || ":._-/".contains(c))
    {
        anyhow::bail!("invalid {label}");
    }
    Ok(())
}

fn validate_metadata(metadata: &LearningMetadata) -> Result<()> {
    validate_identifier(&metadata.model, "model metadata", 128)?;
    if !matches!(
        metadata.reasoning.as_str(),
        "off" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra"
    ) || metadata.prompt_hash.len() != 64
        || !metadata.prompt_hash.bytes().all(|b| b.is_ascii_hexdigit())
    {
        anyhow::bail!("invalid reasoning or prompt hash metadata");
    }
    if let Some(execution) = &metadata.execution {
        validate_identifier(&execution.requested.provider, "requested provider", 64)?;
        validate_identifier(&execution.requested.model, "requested model", 128)?;
        let expected = format!(
            "{}/{}",
            execution.requested.provider, execution.requested.model
        );
        let observed = &execution.observed;
        let identity = match (observed.provider.is_some(), observed.model.is_some()) {
            (true, true) => "verified",
            (false, false) => "unknown",
            _ => "partial",
        };
        if expected != metadata.model
            || execution.requested.provider.contains('/')
            || execution.requested.reasoning != metadata.reasoning
            || execution.identity != identity
            || observed
                .provider
                .as_ref()
                .is_some_and(|v| v != &execution.requested.provider)
            || observed
                .model
                .as_ref()
                .is_some_and(|v| v != &execution.requested.model)
            || observed.fallback_attempts.is_some_and(|v| v != 0)
            || observed
                .backend
                .as_deref()
                .is_some_and(|v| !matches!(v, "openclaw" | "codex" | "claude"))
            || observed.reasoning.as_deref().is_some_and(|v| {
                !matches!(
                    v,
                    "off" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra"
                )
            })
        {
            anyhow::bail!("invalid model execution metadata");
        }
    }
    Ok(())
}

fn validate_grounding(content: &str, quotes: &[&str]) -> Result<()> {
    let (combined, evidence_numbers) = normalize_numeric_evidence(&quotes.join("\n"))?;
    let (content, claim_numbers) = normalize_numeric_evidence(content)?;
    if !claim_numbers.is_subset(&evidence_numbers) {
        anyhow::bail!("learning content contains a number absent from its evidence");
    }
    // These checks are conservative failure shields, not an entailment model.
    // Exact quotation is always the safest way to preserve unfamiliar language.
    for markers in [
        &[
            "not", "never", "no", "without", "isn't", "aren't", "doesn't", "don't", "wasn't",
            "weren't", "cannot", "can't", "没有", "沒有", "并非", "並非", "不是", "不再", "未能",
            "无法", "無法",
        ][..],
        &[
            "if",
            "unless",
            "hypothetical",
            "hypothetically",
            "suppose",
            "supposing",
            "assuming",
            "imagine",
            "might",
            "perhaps",
            "possibly",
            "如果",
            "假设",
            "假設",
            "假如",
            "可能",
            "也许",
            "也許",
        ][..],
        &[
            "previously",
            "formerly",
            "retired",
            "deprecated",
            "discontinued",
            "replaced",
            "no longer",
            "used to",
            "以前",
            "已退役",
            "曾经",
            "曾經",
            "已停用",
            "已退休",
            "已废弃",
            "已廢棄",
        ][..],
    ] {
        if markers.iter().any(|m| has_marker(&combined, m))
            && !markers.iter().any(|m| has_marker(&content, m))
        {
            anyhow::bail!(
                "learning content must preserve evidence negation, uncertainty and historical qualifiers"
            );
        }
    }
    if combined.contains(&content) {
        return Ok(());
    }
    let terms = |text: &str| {
        text.split(|c: char| !c.is_alphanumeric())
            .filter(|s| s.chars().count() >= 3)
            .map(str::to_owned)
            .collect::<BTreeSet<_>>()
    };
    let content_terms = terms(&content);
    let quote_terms = terms(&combined);
    if content_terms.is_empty()
        || content_terms.intersection(&quote_terms).count() * 2 < content_terms.len()
    {
        anyhow::bail!("learning content is insufficiently supported by its exact evidence quotes");
    }
    Ok(())
}

fn normalize_numeric_evidence(value: &str) -> Result<(String, BTreeSet<String>)> {
    let numbers =
        regex::Regex::new(r"[+\-−]?(?:[0-9]+(?:[.,:/+\-−][0-9]+)*|\.[0-9]+)(?:e[+\-−]?[0-9]+)?")?;
    let grouped =
        regex::Regex::new(r"^[+-]?[1-9][0-9]{0,2}(?:,[0-9]{3})+(?:\.[0-9]+)?(?:e[+-]?[0-9]+)?$")?;
    let mut tokens = BTreeSet::new();
    // String tokens preserve signs, versions, dates, exponents and integers
    // beyond floating-point precision. Only valid thousands groups lose commas.
    let lower = value.to_lowercase();
    let normalized = numbers.replace_all(&lower, |capture: &regex::Captures<'_>| {
        let mut token = capture[0].replace('−', "-");
        if grouped.is_match(&token) {
            token = token.replace(',', "");
        }
        tokens.insert(token.clone());
        token
    });
    Ok((normalized.into_owned(), tokens))
}

fn has_marker(text: &str, marker: &str) -> bool {
    if !marker.is_ascii() || marker.contains(' ') {
        text.contains(marker)
    } else {
        text.split(|c: char| !c.is_alphanumeric() && c != '\'')
            .any(|word| word == marker)
    }
}
