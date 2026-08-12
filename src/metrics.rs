use crate::model::{
    ContextMetricRecord, ContextObservation, ContextRequest, MetricsSummary, ReviewOutcome,
    RuntimeMetricInput, RuntimeMetricRecord, RuntimeMetricsSummary,
};
use crate::store::{Store, create_private_dir_all, now_ms, set_private_file};
use crate::{EmbedReport, EmbeddingProvider};
use anyhow::{Context, Result};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::Instant;

impl Store {
    pub fn observe_context(
        &self,
        request: &ContextRequest,
        provider: Option<&dyn EmbeddingProvider>,
    ) -> Result<ContextObservation> {
        let request_id = format!("{:032x}", rand::random::<u128>());
        let occurred_at_ms = now_ms();
        let started = Instant::now();
        let result = self.assemble_context(request, provider);
        let duration_us = elapsed_us(started);

        match result {
            Ok(packet) => {
                let recorded = self
                    .connection
                    .execute(
                        "INSERT INTO context_metrics(
                             request_id, occurred_at_ms, retrieval_mode, status, duration_us,
                             memory_count, reference_count, packet_chars, packet_truncated
                         ) VALUES(?1, ?2, ?3, 'ok', ?4, ?5, ?6, ?7, ?8)",
                        params![
                            request_id,
                            occurred_at_ms,
                            request.mode.as_str(),
                            duration_us,
                            packet.memories.len() as i64,
                            packet.references.len() as i64,
                            if packet.is_empty() {
                                0
                            } else {
                                packet.used_chars as i64
                            },
                            packet.truncated,
                        ],
                    )
                    .is_ok();
                Ok(ContextObservation {
                    request_id: recorded.then_some(request_id),
                    packet,
                })
            }
            Err(error) => {
                let _ = self.connection.execute(
                    "INSERT INTO context_metrics(
                         request_id, occurred_at_ms, retrieval_mode, status, duration_us
                     ) VALUES(?1, ?2, ?3, 'error', ?4)",
                    params![
                        request_id,
                        occurred_at_ms,
                        request.mode.as_str(),
                        duration_us,
                    ],
                );
                Err(error)
            }
        }
    }

    pub fn mark_context_injected(&self, request_id: &str, injected: bool) -> Result<()> {
        validate_request_id(request_id)?;
        let updated = self.connection.execute(
            "UPDATE context_metrics
             SET adapter_injected = ?2
             WHERE request_id = ?1 AND status = 'ok'",
            params![request_id, injected],
        )?;
        if updated == 0 {
            anyhow::bail!("unknown successful context metric request id");
        }
        Ok(())
    }

    pub fn review_context_metric(
        &self,
        request_id: &str,
        outcome: ReviewOutcome,
        expected_rank: Option<usize>,
    ) -> Result<ContextMetricRecord> {
        validate_request_id(request_id)?;
        if expected_rank == Some(0) {
            anyhow::bail!("expected rank must be greater than zero");
        }
        let expected_rank = expected_rank
            .map(i64::try_from)
            .transpose()
            .context("expected rank is too large")?;
        let updated = self.connection.execute(
            "UPDATE context_metrics
             SET review_outcome = ?2, expected_rank = ?3, reviewed_at_ms = ?4
             WHERE request_id = ?1",
            params![request_id, outcome.as_str(), expected_rank, now_ms()],
        )?;
        if updated == 0 {
            anyhow::bail!("unknown context metric request id");
        }
        self.context_metric(request_id)?
            .context("reviewed context metric disappeared")
    }

    pub fn context_metrics_recent(
        &self,
        since_ms: i64,
        limit: usize,
    ) -> Result<Vec<ContextMetricRecord>> {
        let mut statement = self.connection.prepare(
            "SELECT request_id, occurred_at_ms, retrieval_mode, status, duration_us,
                    memory_count, reference_count, packet_chars, packet_truncated,
                    adapter_injected, review_outcome, expected_rank, reviewed_at_ms
             FROM context_metrics
             WHERE occurred_at_ms >= ?1
             ORDER BY occurred_at_ms DESC, request_id DESC
             LIMIT ?2",
        )?;
        let rows = statement
            .query_map(
                params![since_ms, limit.clamp(1, 10_000) as i64],
                metric_from_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn metrics_summary(&self, since_ms: i64) -> Result<MetricsSummary> {
        let until_ms = now_ms();
        let mut statement = self.connection.prepare(
            "SELECT status, duration_us, memory_count, reference_count, packet_chars,
                    packet_truncated, adapter_injected, review_outcome, expected_rank
             FROM context_metrics
             WHERE occurred_at_ms >= ?1 AND occurred_at_ms <= ?2
             ORDER BY duration_us",
        )?;
        let rows = statement
            .query_map(params![since_ms, until_ms], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, bool>(5)?,
                    row.get::<_, Option<bool>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<i64>>(8)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut successful_requests = 0usize;
        let mut failed_requests = 0usize;
        let mut empty_packet_candidates = 0usize;
        let mut injection_observed = 0usize;
        let mut injected_packets = 0usize;
        let mut truncated_packets = 0usize;
        let mut reviewed_requests = 0usize;
        let mut expected_rank_samples = 0usize;
        let mut expected_top_three = 0usize;
        let mut successful_packet_chars = 0u128;
        let mut latencies = Vec::with_capacity(rows.len());
        let mut review_outcomes = ReviewOutcome::ALL
            .into_iter()
            .map(|outcome| (outcome.as_str().to_string(), 0usize))
            .collect::<BTreeMap<_, _>>();

        for (
            status,
            duration_us,
            memory_count,
            reference_count,
            packet_chars,
            packet_truncated,
            adapter_injected,
            review_outcome,
            expected_rank,
        ) in rows
        {
            latencies.push(duration_us as f64 / 1_000.0);
            if status == "ok" {
                successful_requests += 1;
                successful_packet_chars += packet_chars as u128;
                if memory_count + reference_count == 0 {
                    empty_packet_candidates += 1;
                }
                if packet_truncated {
                    truncated_packets += 1;
                }
            } else {
                failed_requests += 1;
            }
            if let Some(injected) = adapter_injected {
                injection_observed += 1;
                if injected {
                    injected_packets += 1;
                }
            }
            if let Some(outcome) = review_outcome {
                reviewed_requests += 1;
                if let Some(count) = review_outcomes.get_mut(&outcome) {
                    *count += 1;
                }
            }
            if let Some(rank) = expected_rank {
                expected_rank_samples += 1;
                if rank <= 3 {
                    expected_top_three += 1;
                }
            }
        }

        let context_requests = successful_requests + failed_requests;
        Ok(MetricsSummary {
            since_ms,
            until_ms,
            context_requests,
            successful_requests,
            failed_requests,
            empty_packet_candidates,
            injection_observed,
            injected_packets,
            injection_rate: ratio(injected_packets, injection_observed),
            truncated_packets,
            truncation_rate: ratio(truncated_packets, successful_requests),
            reviewed_requests,
            review_outcomes,
            expected_rank_samples,
            expected_top_three_rate: ratio(expected_top_three, expected_rank_samples),
            average_packet_chars: (successful_requests > 0)
                .then(|| successful_packet_chars as f64 / successful_requests as f64),
            p50_ms: percentile(&latencies, 0.50),
            p95_ms: percentile(&latencies, 0.95),
            p99_ms: percentile(&latencies, 0.99),
            runtime: self.runtime_metrics_summary(since_ms, until_ms)?,
        })
    }

    pub fn prune_metrics(&self, before_ms: i64, apply: bool) -> Result<usize> {
        let context_count = self.connection.query_row(
            "SELECT count(*) FROM context_metrics WHERE occurred_at_ms < ?1",
            [before_ms],
            |row| row.get::<_, i64>(0),
        )? as usize;
        let runtime_count = self.connection.query_row(
            "SELECT count(*) FROM runtime_metrics WHERE occurred_at_ms < ?1",
            [before_ms],
            |row| row.get::<_, i64>(0),
        )? as usize;
        let count = context_count + runtime_count;
        if apply && count > 0 {
            let transaction = self.connection.unchecked_transaction()?;
            transaction.execute(
                "DELETE FROM context_metrics WHERE occurred_at_ms < ?1",
                [before_ms],
            )?;
            transaction.execute(
                "DELETE FROM runtime_metrics WHERE occurred_at_ms < ?1",
                [before_ms],
            )?;
            transaction.commit()?;
        }
        Ok(count)
    }

    pub fn export_metrics(&self, destination: &Path, since_ms: i64) -> Result<usize> {
        if destination.exists() {
            anyhow::bail!(
                "metrics export destination already exists: {}",
                destination.display()
            );
        }
        if let Some(parent) = destination.parent() {
            create_private_dir_all(parent)?;
        }
        let records = self.all_context_metrics_since(since_ms)?;
        let runtime_records = self.runtime_metrics_since(since_ms)?;
        #[derive(Serialize)]
        struct Export<'a> {
            schema: u32,
            redacted: bool,
            generated_at_ms: i64,
            since_ms: i64,
            records: &'a [ContextMetricRecord],
            runtime_records: &'a [RuntimeMetricRecord],
        }
        let output = serde_json::to_vec_pretty(&Export {
            schema: 1,
            redacted: true,
            generated_at_ms: now_ms(),
            since_ms,
            records: &records,
            runtime_records: &runtime_records,
        })?;
        fs::write(destination, output)
            .with_context(|| format!("failed to write {}", destination.display()))?;
        set_private_file(destination)?;
        Ok(records.len() + runtime_records.len())
    }

    pub fn observe_embeddings(
        &mut self,
        provider: &dyn EmbeddingProvider,
        limit: usize,
    ) -> Result<EmbedReport> {
        let started = Instant::now();
        let result = self.embed_pending(provider, limit);
        let input = match &result {
            Ok(report) => RuntimeMetricInput {
                event_kind: "embedding".to_string(),
                status: "ok".to_string(),
                duration_us: elapsed_us(started) as u64,
                embedding_selected: Some(report.selected),
                embedding_completed: Some(report.embedded),
                embedding_remaining: Some(report.remaining),
                ..RuntimeMetricInput::default()
            },
            Err(_) => RuntimeMetricInput {
                event_kind: "embedding".to_string(),
                status: "error".to_string(),
                duration_us: elapsed_us(started) as u64,
                embedding_selected: Some(0),
                embedding_completed: Some(0),
                ..RuntimeMetricInput::default()
            },
        };
        let _ = self.record_runtime_metric(&input);
        result
    }

    pub fn record_runtime_metric(&self, input: &RuntimeMetricInput) -> Result<String> {
        validate_runtime_metric(input)?;
        let event_id = format!("{:032x}", rand::random::<u128>());
        self.connection.execute(
            "INSERT INTO runtime_metrics(
                 event_id, occurred_at_ms, event_kind, status, duration_us,
                 evidence_changed, learning_eligible, proposed_memories, accepted_memories,
                 embedding_selected, embedding_completed, embedding_remaining,
                 compacted, tokens_before, tokens_after
             ) VALUES(
                 ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15
             )",
            params![
                event_id,
                now_ms(),
                input.event_kind,
                input.status,
                i64::try_from(input.duration_us).unwrap_or(i64::MAX),
                input.evidence_changed,
                input.learning_eligible,
                optional_i64(input.proposed_memories)?,
                optional_i64(input.accepted_memories)?,
                optional_i64(input.embedding_selected)?,
                optional_i64(input.embedding_completed)?,
                optional_i64(input.embedding_remaining)?,
                input.compacted,
                optional_i64(input.tokens_before)?,
                optional_i64(input.tokens_after)?,
            ],
        )?;
        Ok(event_id)
    }

    fn all_context_metrics_since(&self, since_ms: i64) -> Result<Vec<ContextMetricRecord>> {
        let mut statement = self.connection.prepare(
            "SELECT request_id, occurred_at_ms, retrieval_mode, status, duration_us,
                    memory_count, reference_count, packet_chars, packet_truncated,
                    adapter_injected, review_outcome, expected_rank, reviewed_at_ms
             FROM context_metrics
             WHERE occurred_at_ms >= ?1
             ORDER BY occurred_at_ms DESC, request_id DESC",
        )?;
        Ok(statement
            .query_map([since_ms], metric_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn runtime_metrics_since(&self, since_ms: i64) -> Result<Vec<RuntimeMetricRecord>> {
        let mut statement = self.connection.prepare(
            "SELECT event_id, occurred_at_ms, event_kind, status, duration_us,
                    evidence_changed, learning_eligible, proposed_memories, accepted_memories,
                    embedding_selected, embedding_completed, embedding_remaining,
                    compacted, tokens_before, tokens_after
             FROM runtime_metrics
             WHERE occurred_at_ms >= ?1
             ORDER BY occurred_at_ms DESC, event_id DESC",
        )?;
        Ok(statement
            .query_map([since_ms], runtime_metric_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn runtime_metrics_summary(
        &self,
        since_ms: i64,
        until_ms: i64,
    ) -> Result<RuntimeMetricsSummary> {
        let records = self.runtime_metrics_since(since_ms)?;
        let mut summary = RuntimeMetricsSummary {
            learning_events: 0,
            learning_failures: 0,
            evidence_records: 0,
            eligible_turns: 0,
            proposed_memories: 0,
            accepted_memories: 0,
            embedding_events: 0,
            embedding_failures: 0,
            embeddings_selected: 0,
            embeddings_completed: 0,
            latest_embedding_remaining: None,
            compaction_events: 0,
            compaction_failures: 0,
            completed_compactions: 0,
        };
        for record in records
            .into_iter()
            .filter(|record| record.occurred_at_ms <= until_ms)
        {
            match record.event_kind.as_str() {
                "learning" => {
                    summary.learning_events += 1;
                    summary.learning_failures += usize::from(record.status == "error");
                    summary.evidence_records += usize::from(record.evidence_changed == Some(true));
                    summary.eligible_turns += usize::from(record.learning_eligible == Some(true));
                    summary.proposed_memories += record.proposed_memories.unwrap_or(0);
                    summary.accepted_memories += record.accepted_memories.unwrap_or(0);
                }
                "embedding" => {
                    summary.embedding_events += 1;
                    summary.embedding_failures += usize::from(record.status == "error");
                    summary.embeddings_selected += record.embedding_selected.unwrap_or(0);
                    summary.embeddings_completed += record.embedding_completed.unwrap_or(0);
                    if summary.latest_embedding_remaining.is_none() {
                        summary.latest_embedding_remaining = record.embedding_remaining;
                    }
                }
                "compaction" => {
                    summary.compaction_events += 1;
                    summary.compaction_failures += usize::from(record.status == "error");
                    summary.completed_compactions += usize::from(record.compacted == Some(true));
                }
                _ => unreachable!("validated runtime metric kind"),
            }
        }
        Ok(summary)
    }

    fn context_metric(&self, request_id: &str) -> Result<Option<ContextMetricRecord>> {
        self.connection
            .query_row(
                "SELECT request_id, occurred_at_ms, retrieval_mode, status, duration_us,
                        memory_count, reference_count, packet_chars, packet_truncated,
                        adapter_injected, review_outcome, expected_rank, reviewed_at_ms
                 FROM context_metrics
                 WHERE request_id = ?1",
                [request_id],
                metric_from_row,
            )
            .optional()
            .map_err(Into::into)
    }
}

fn metric_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ContextMetricRecord> {
    Ok(ContextMetricRecord {
        request_id: row.get(0)?,
        occurred_at_ms: row.get(1)?,
        retrieval_mode: row.get(2)?,
        status: row.get(3)?,
        duration_us: row.get::<_, i64>(4)? as u64,
        memory_count: row.get::<_, i64>(5)? as usize,
        reference_count: row.get::<_, i64>(6)? as usize,
        packet_chars: row.get::<_, i64>(7)? as usize,
        packet_truncated: row.get(8)?,
        adapter_injected: row.get(9)?,
        review_outcome: row.get(10)?,
        expected_rank: row.get::<_, Option<i64>>(11)?.map(|rank| rank as usize),
        reviewed_at_ms: row.get(12)?,
    })
}

fn runtime_metric_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RuntimeMetricRecord> {
    Ok(RuntimeMetricRecord {
        event_id: row.get(0)?,
        occurred_at_ms: row.get(1)?,
        event_kind: row.get(2)?,
        status: row.get(3)?,
        duration_us: row.get::<_, i64>(4)? as u64,
        evidence_changed: row.get(5)?,
        learning_eligible: row.get(6)?,
        proposed_memories: row.get::<_, Option<i64>>(7)?.map(|value| value as usize),
        accepted_memories: row.get::<_, Option<i64>>(8)?.map(|value| value as usize),
        embedding_selected: row.get::<_, Option<i64>>(9)?.map(|value| value as usize),
        embedding_completed: row.get::<_, Option<i64>>(10)?.map(|value| value as usize),
        embedding_remaining: row.get::<_, Option<i64>>(11)?.map(|value| value as usize),
        compacted: row.get(12)?,
        tokens_before: row.get::<_, Option<i64>>(13)?.map(|value| value as usize),
        tokens_after: row.get::<_, Option<i64>>(14)?.map(|value| value as usize),
    })
}

fn validate_runtime_metric(input: &RuntimeMetricInput) -> Result<()> {
    if !matches!(
        input.event_kind.as_str(),
        "learning" | "embedding" | "compaction"
    ) {
        anyhow::bail!("unknown runtime metric kind");
    }
    if !matches!(input.status.as_str(), "ok" | "error" | "skipped") {
        anyhow::bail!("unknown runtime metric status");
    }
    let has_learning = input.evidence_changed.is_some()
        || input.learning_eligible.is_some()
        || input.proposed_memories.is_some()
        || input.accepted_memories.is_some();
    let has_embedding = input.embedding_selected.is_some()
        || input.embedding_completed.is_some()
        || input.embedding_remaining.is_some();
    let has_compaction =
        input.compacted.is_some() || input.tokens_before.is_some() || input.tokens_after.is_some();
    let fields_match = match input.event_kind.as_str() {
        "learning" => has_learning && !has_embedding && !has_compaction,
        "embedding" => has_embedding && !has_learning && !has_compaction,
        "compaction" => has_compaction && !has_learning && !has_embedding,
        _ => false,
    };
    if !fields_match {
        anyhow::bail!("runtime metric fields do not match its event kind");
    }
    Ok(())
}

fn optional_i64(value: Option<usize>) -> Result<Option<i64>> {
    value
        .map(i64::try_from)
        .transpose()
        .context("runtime metric count is too large")
}

fn elapsed_us(started: Instant) -> i64 {
    i64::try_from(started.elapsed().as_micros()).unwrap_or(i64::MAX)
}

fn validate_request_id(request_id: &str) -> Result<()> {
    if request_id.len() != 32
        || !request_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        anyhow::bail!("context metric request id must be 32 lowercase hexadecimal characters");
    }
    Ok(())
}

fn ratio(numerator: usize, denominator: usize) -> Option<f64> {
    (denominator > 0).then(|| numerator as f64 / denominator as f64)
}

fn percentile(sorted: &[f64], fraction: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let index = ((sorted.len() - 1) as f64 * fraction).round() as usize;
    Some(sorted[index])
}
