use crate::chunking::{TextChunk, chunk_text, sha256_hex};
use crate::embedding::{EmbeddingProvider, vector_to_blob};
use crate::memory::fuzzy_context_score;
use crate::model::{
    EmbedReport, HealthReport, IngestDocument, IngestOutcome, MemoryInput, SearchHit, SearchMode,
    SearchRequest,
};
use crate::redaction::redact_text;
use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::backup::Backup;
use rusqlite::types::Value;
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
    params_from_iter,
};
use sqlite_vec::sqlite3_vec_init;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Once;
use std::time::Duration;

const SCHEMA_VERSION: i64 = 6;
const DEFAULT_CHUNK_CHARS: usize = 1_400;
const DEFAULT_CHUNK_OVERLAP_CHARS: usize = 180;
const MAX_DOCUMENT_BYTES: usize = 8 * 1024 * 1024;
const MAX_METADATA_BYTES: usize = 256 * 1024;
const MAX_TITLE_BYTES: usize = 4 * 1024;
const MAX_QUERY_BYTES: usize = 16 * 1024;
const MAX_EMBED_BATCH_CHARS: usize = 100_000;
const EMBEDDING_LEASE_MS: i64 = 120_000;
const MAX_EMBED_ATTEMPTS: i64 = 5;
const RRF_K: f64 = 60.0;

static REGISTER_VECTOR_EXTENSION: Once = Once::new();

pub struct Store {
    path: PathBuf,
    pub(crate) connection: Connection,
    embedding_dimensions: usize,
}

pub(crate) struct PreparedIngest {
    document: IngestDocument,
    content_hash: String,
    chunks: Vec<TextChunk>,
}

impl Store {
    pub fn open(path: impl AsRef<Path>, embedding_dimensions: usize) -> Result<Self> {
        validate_dimensions(embedding_dimensions)?;
        register_vector_extension();
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            create_private_dir_all(parent)?;
            if parent.file_name().is_some_and(|name| name == "state") {
                set_private_dir(parent)?;
            }
        }
        let connection = Connection::open(&path)
            .with_context(|| format!("failed to open {}", path.display()))?;
        set_private_file(&path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA synchronous = NORMAL;
             PRAGMA temp_store = MEMORY;
             PRAGMA wal_autocheckpoint = 1000;
             PRAGMA mmap_size = 268435456;",
        )?;
        let journal_mode =
            connection.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))?;
        if !journal_mode.eq_ignore_ascii_case("wal") {
            connection.pragma_update(None, "journal_mode", "WAL")?;
        }
        set_private_sqlite_sidecars(&path)?;

        let mut store = Self {
            path,
            connection,
            embedding_dimensions,
        };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_existing(path: impl AsRef<Path>, embedding_dimensions: usize) -> Result<Self> {
        validate_dimensions(embedding_dimensions)?;
        register_vector_extension();
        let path = path.as_ref().to_path_buf();
        if !path.is_file() {
            anyhow::bail!("Moon database does not exist: {}", path.display());
        }
        let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| format!("failed to open existing database {}", path.display()))?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA temp_store = MEMORY;
             PRAGMA mmap_size = 268435456;",
        )?;
        let schema_version =
            connection.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?;
        if schema_version != SCHEMA_VERSION {
            anyhow::bail!(
                "database schema version is {schema_version}; expected {SCHEMA_VERSION}; run `moon init` to migrate it"
            );
        }
        let existing_dimensions = connection
            .query_row(
                "SELECT value FROM metadata WHERE key = 'embedding_dimensions'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|value| value.parse::<usize>())
            .transpose()
            .context("invalid embedding_dimensions database metadata")?
            .ok_or_else(|| anyhow::anyhow!("database is missing embedding_dimensions metadata"))?;
        if existing_dimensions != embedding_dimensions {
            anyhow::bail!(
                "embedding dimensions mismatch: database={existing_dimensions} requested={embedding_dimensions}"
            );
        }
        Ok(Self {
            path,
            connection,
            embedding_dimensions,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn embedding_dimensions(&self) -> usize {
        self.embedding_dimensions
    }

    fn migrate(&mut self) -> Result<()> {
        let current_version = self
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?;
        if current_version == SCHEMA_VERSION {
            let dimensions = self
                .metadata_value("embedding_dimensions")?
                .ok_or_else(|| {
                    anyhow::anyhow!("database is missing embedding_dimensions metadata")
                })?
                .parse::<usize>()
                .context("invalid embedding_dimensions database metadata")?;
            if dimensions != self.embedding_dimensions {
                anyhow::bail!(
                    "embedding dimensions mismatch: database={dimensions} requested={}",
                    self.embedding_dimensions
                );
            }
            let vector_table_exists = self.connection.query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM sqlite_master
                     WHERE type = 'table' AND name = 'chunk_vectors'
                 )",
                [],
                |row| row.get::<_, bool>(0),
            )?;
            if !vector_table_exists {
                anyhow::bail!("database schema is incomplete: chunk_vectors is missing");
            }
            return Ok(());
        }
        let transaction = self.connection.transaction()?;
        let existing_version =
            transaction.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?;
        if existing_version > SCHEMA_VERSION {
            anyhow::bail!(
                "database schema version {existing_version} is newer than supported version {SCHEMA_VERSION}"
            );
        }
        if existing_version < 1 {
            transaction.execute_batch(include_str!("../migrations/0001_init.sql"))?;
            transaction.execute(
                "INSERT OR IGNORE INTO schema_migrations(version, applied_at_ms) VALUES(1, ?1)",
                [now_ms()],
            )?;
        }
        if existing_version < 2 {
            transaction.execute_batch(include_str!("../migrations/0002_canonical_body.sql"))?;
            transaction.execute(
                "INSERT OR IGNORE INTO schema_migrations(version, applied_at_ms) VALUES(2, ?1)",
                [now_ms()],
            )?;
        }
        if existing_version < 3 {
            transaction.execute_batch(include_str!("../migrations/0003_fts_source_kind.sql"))?;
            transaction.execute(
                "INSERT OR IGNORE INTO schema_migrations(version, applied_at_ms) VALUES(3, ?1)",
                [now_ms()],
            )?;
        }
        if existing_version < 4 {
            transaction.execute_batch(include_str!("../migrations/0004_evidence_memory.sql"))?;
            transaction.execute(
                "INSERT OR IGNORE INTO schema_migrations(version, applied_at_ms) VALUES(4, ?1)",
                [now_ms()],
            )?;
        }
        if existing_version < 5 {
            transaction.execute_batch(include_str!("../migrations/0005_runtime_safety.sql"))?;
            transaction.execute(
                "INSERT OR IGNORE INTO schema_migrations(version, applied_at_ms) VALUES(5, ?1)",
                [now_ms()],
            )?;
        }
        if existing_version < 6 {
            transaction.execute_batch(include_str!("../migrations/0006_auto_embedding.sql"))?;
            transaction.execute(
                "INSERT OR IGNORE INTO schema_migrations(version, applied_at_ms) VALUES(6, ?1)",
                [now_ms()],
            )?;
        }

        let existing_dimensions = transaction
            .query_row(
                "SELECT value FROM metadata WHERE key = 'embedding_dimensions'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|value| value.parse::<usize>())
            .transpose()
            .context("invalid embedding_dimensions database metadata")?;
        if let Some(existing) = existing_dimensions {
            if existing != self.embedding_dimensions {
                anyhow::bail!(
                    "embedding dimensions mismatch: database={existing} requested={}; use a separate database or rebuild embeddings",
                    self.embedding_dimensions
                );
            }
        } else {
            transaction.execute(
                "INSERT INTO metadata(key, value) VALUES('embedding_dimensions', ?1)",
                [self.embedding_dimensions.to_string()],
            )?;
        }

        transaction.execute(
            "INSERT OR IGNORE INTO metadata(key, value) VALUES('engine', 'moon')",
            [],
        )?;
        transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;

        let vector_schema = format!(
            "CREATE VIRTUAL TABLE IF NOT EXISTS chunk_vectors USING vec0(
                 embedding float[{}],
                 source_kind text,
                 scope text
             )",
            self.embedding_dimensions
        );
        transaction.execute_batch(&vector_schema)?;
        if existing_version < 6 {
            transaction.execute(
                "DELETE FROM chunk_vectors
                 WHERE rowid IN (
                     SELECT c.id
                     FROM chunks c
                     JOIN documents d ON d.id = c.document_id
                     WHERE d.source_kind = 'evidence'
                 )",
                [],
            )?;
            transaction.execute(
                "UPDATE chunks
                 SET embedding_model = NULL, embedded_at_ms = NULL
                 WHERE document_id IN (
                     SELECT id FROM documents WHERE source_kind = 'evidence'
                 )",
                [],
            )?;
        }
        if existing_version < 5 {
            let now = now_ms();
            transaction.execute(
                "DELETE FROM chunk_fts
                 WHERE rowid IN (
                     SELECT c.id
                     FROM chunks c
                     JOIN documents d ON d.id = c.document_id
                     JOIN memory_items m ON m.document_id = d.id
                     WHERE (
                           m.superseded_by IS NOT NULL
                           OR (m.valid_until_ms IS NOT NULL AND m.valid_until_ms <= ?1)
                       )
                 )",
                [now],
            )?;
            transaction.execute(
                "DELETE FROM chunk_vectors
                 WHERE rowid IN (
                     SELECT c.id
                     FROM chunks c
                     JOIN documents d ON d.id = c.document_id
                     JOIN memory_items m ON m.document_id = d.id
                     WHERE (
                           m.superseded_by IS NOT NULL
                           OR (m.valid_until_ms IS NOT NULL AND m.valid_until_ms <= ?1)
                       )
                 )",
                [now],
            )?;
            transaction.execute(
                "DELETE FROM embedding_queue
                 WHERE chunk_id IN (
                     SELECT c.id
                     FROM chunks c
                     JOIN documents d ON d.id = c.document_id
                     JOIN memory_items m ON m.document_id = d.id
                     WHERE (
                           m.superseded_by IS NOT NULL
                           OR (m.valid_until_ms IS NOT NULL AND m.valid_until_ms <= ?1)
                       )
                 )",
                [now],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn ingest(&mut self, document: IngestDocument) -> Result<IngestOutcome> {
        let prepared = prepare_ingest(document)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let outcome = ingest_prepared(&transaction, prepared)?;
        transaction.commit()?;
        Ok(outcome)
    }

    pub fn remember(&mut self, input: MemoryInput) -> Result<IngestOutcome> {
        validate_unit_interval("importance", input.importance)?;
        validate_unit_interval("confidence", input.confidence)?;
        let timestamp = now_ms();
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let nonce = random_nonce(&transaction)?;
        let prepared = prepare_ingest(IngestDocument {
            source_uri: format!("memory://adhoc/{nonce}"),
            source_kind: "memory".to_string(),
            scope: input.scope,
            title: input.title,
            content: input.content,
            modified_at_ms: timestamp,
            metadata_json: "{}".to_string(),
        })?;
        let outcome = ingest_prepared(&transaction, prepared)?;
        transaction.execute(
            "INSERT INTO memory_items(
                 document_id, memory_kind, importance, confidence, valid_from_ms, pinned
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                outcome.document_id,
                input.memory_kind,
                input.importance,
                input.confidence,
                timestamp,
                input.pinned,
            ],
        )?;
        transaction.commit()?;
        Ok(outcome)
    }

    pub fn embed_pending(
        &mut self,
        provider: &dyn EmbeddingProvider,
        limit: usize,
    ) -> Result<EmbedReport> {
        if provider.dimensions() != self.embedding_dimensions {
            anyhow::bail!(
                "embedding dimensions mismatch: database={} provider={}",
                self.embedding_dimensions,
                provider.dimensions()
            );
        }
        let claimed_at = now_ms();
        let lease_until = claimed_at + EMBEDDING_LEASE_MS;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let active_model = transaction
            .query_row(
                "SELECT value FROM metadata WHERE key = 'embedding_model'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if let Some(active_model) = active_model.as_deref()
            && active_model != provider.model()
        {
            anyhow::bail!(
                "embedding model mismatch: database={active_model} provider={}; run requeue-embeddings before changing models",
                provider.model()
            );
        }
        let worker_id = random_nonce(&transaction)?;
        let mut selected = {
            let mut statement = transaction.prepare(
                "SELECT q.chunk_id, c.content, d.source_kind, d.scope
                 FROM embedding_queue q
                 JOIN chunks c ON c.id = q.chunk_id
                 JOIN documents d ON d.id = c.document_id
                 WHERE d.active = 1
                   AND (q.lease_until_ms IS NULL OR q.lease_until_ms <= ?1)
                   AND (q.next_attempt_at_ms IS NULL OR q.next_attempt_at_ms <= ?1)
                   AND q.attempts < ?2
                 ORDER BY q.priority DESC, q.queued_at_ms, q.chunk_id
                 LIMIT ?3",
            )?;
            statement
                .query_map(
                    params![claimed_at, MAX_EMBED_ATTEMPTS, limit.clamp(1, 1_000) as i64],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    },
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut batch_chars = 0usize;
        selected.retain(|(_, content, _, _)| {
            let content_chars = content.chars().count();
            if batch_chars > 0 && batch_chars.saturating_add(content_chars) > MAX_EMBED_BATCH_CHARS
            {
                return false;
            }
            batch_chars = batch_chars.saturating_add(content_chars);
            true
        });
        for (chunk_id, _, _, _) in &selected {
            transaction.execute(
                "UPDATE embedding_queue
                 SET claimed_by = ?2, lease_until_ms = ?3,
                     attempts = attempts + 1, last_attempt_at_ms = ?4,
                     next_attempt_at_ms = NULL, last_error = NULL
                 WHERE chunk_id = ?1
                   AND (lease_until_ms IS NULL OR lease_until_ms <= ?4)
                   AND (next_attempt_at_ms IS NULL OR next_attempt_at_ms <= ?4)
                   AND attempts < ?5",
                params![
                    chunk_id,
                    worker_id,
                    lease_until,
                    claimed_at,
                    MAX_EMBED_ATTEMPTS
                ],
            )?;
        }
        if !selected.is_empty() {
            transaction.execute(
                "INSERT INTO metadata(key, value) VALUES('embedding_model', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [provider.model()],
            )?;
        }
        transaction.commit()?;

        if selected.is_empty() {
            return Ok(EmbedReport {
                provider: provider.name().to_string(),
                model: provider.model().to_string(),
                selected: 0,
                embedded: 0,
                remaining: self.count("embedding_queue")?,
            });
        }

        let inputs = selected
            .iter()
            .map(|(_, content, _, _)| content.clone())
            .collect::<Vec<_>>();
        let vectors = match provider.embed_documents(&inputs).and_then(|vectors| {
            validate_vectors(vectors, selected.len(), self.embedding_dimensions)
        }) {
            Ok(vectors) => vectors,
            Err(error) => {
                self.release_embedding_claims(&worker_id, &selected, &error)?;
                return Err(error);
            }
        };

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let active_model = transaction
            .query_row(
                "SELECT value FROM metadata WHERE key = 'embedding_model'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if active_model.as_deref() != Some(provider.model()) {
            anyhow::bail!(
                "embedding model changed while a batch was in flight; results were not written"
            );
        }
        let mut embedded = 0usize;
        for ((chunk_id, _, source_kind, scope), vector) in selected.iter().zip(vectors) {
            let still_claimed = transaction.query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM embedding_queue
                     WHERE chunk_id = ?1 AND claimed_by = ?2
                 )",
                params![chunk_id, worker_id],
                |row| row.get::<_, bool>(0),
            )?;
            if !still_claimed {
                continue;
            }
            transaction.execute(
                "INSERT OR REPLACE INTO chunk_vectors(rowid, embedding, source_kind, scope)
                 VALUES(?1, ?2, ?3, ?4)",
                params![chunk_id, vector_to_blob(&vector), source_kind, scope],
            )?;
            transaction.execute(
                "UPDATE chunks SET embedding_model = ?2, embedded_at_ms = ?3 WHERE id = ?1",
                params![chunk_id, provider.model(), now_ms()],
            )?;
            transaction.execute(
                "DELETE FROM embedding_queue WHERE chunk_id = ?1 AND claimed_by = ?2",
                params![chunk_id, worker_id],
            )?;
            embedded += 1;
        }
        transaction.commit()?;
        let remaining = self.count("embedding_queue")?;
        Ok(EmbedReport {
            provider: provider.name().to_string(),
            model: provider.model().to_string(),
            selected: selected.len(),
            embedded,
            remaining,
        })
    }

    fn release_embedding_claims(
        &mut self,
        worker_id: &str,
        selected: &[(i64, String, String, String)],
        error: &anyhow::Error,
    ) -> Result<()> {
        let message = sanitized_error(error);
        let failed_at = now_ms();
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        for (chunk_id, _, _, _) in selected {
            transaction.execute(
                "UPDATE embedding_queue
                 SET claimed_by = NULL, lease_until_ms = NULL, last_error = ?3
                     , next_attempt_at_ms = ?4 + CASE attempts
                         WHEN 1 THEN 5000
                         WHEN 2 THEN 30000
                         WHEN 3 THEN 120000
                         ELSE 600000
                       END
                 WHERE chunk_id = ?1 AND claimed_by = ?2",
                params![chunk_id, worker_id, message, failed_at],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn search(
        &self,
        request: &SearchRequest,
        provider: Option<&dyn EmbeddingProvider>,
    ) -> Result<Vec<SearchHit>> {
        if request.query.trim().is_empty() {
            anyhow::bail!("search query must not be empty");
        }
        if request.query.len() > MAX_QUERY_BYTES {
            anyhow::bail!("search query exceeds the maximum size of {MAX_QUERY_BYTES} bytes");
        }
        let limit = request.limit.clamp(1, 100);
        match request.mode {
            SearchMode::Lexical => self.lexical_search(request, limit),
            SearchMode::Semantic => {
                let vector = self.embed_query(provider, &request.query)?;
                self.semantic_search(request, &vector, limit)
            }
            SearchMode::Hybrid => {
                let lexical = self.lexical_search(request, limit.saturating_mul(4))?;
                let vector = self.embed_query(provider, &request.query)?;
                let semantic = self.semantic_search(request, &vector, limit.saturating_mul(4))?;
                Ok(fuse_rankings(lexical, semantic, limit))
            }
        }
    }

    fn embed_query(
        &self,
        provider: Option<&dyn EmbeddingProvider>,
        query: &str,
    ) -> Result<Vec<f32>> {
        let provider = provider.ok_or_else(|| {
            anyhow::anyhow!("semantic and hybrid search require an embedding provider")
        })?;
        if provider.dimensions() != self.embedding_dimensions {
            anyhow::bail!(
                "embedding dimensions mismatch: database={} provider={}",
                self.embedding_dimensions,
                provider.dimensions()
            );
        }
        if let Some(active_model) = self.metadata_value("embedding_model")?
            && active_model != provider.model()
        {
            anyhow::bail!(
                "query embedding model mismatch: database={active_model} provider={}",
                provider.model()
            );
        }
        let vector = provider.embed_query(query)?;
        if vector.len() != self.embedding_dimensions {
            anyhow::bail!(
                "query embedding has {} dimensions; expected {}",
                vector.len(),
                self.embedding_dimensions
            );
        }
        if vector.iter().any(|value| !value.is_finite()) {
            anyhow::bail!("query embedding contains a non-finite value");
        }
        Ok(vector)
    }

    fn lexical_search(&self, request: &SearchRequest, limit: usize) -> Result<Vec<SearchHit>> {
        let (strict_query, relaxed_query) = safe_fts_queries(&request.query)?;
        let hits = self.lexical_search_query(request, limit, &strict_query)?;
        if hits.is_empty() && relaxed_query != strict_query {
            let relaxed = self.lexical_search_query(request, limit, &relaxed_query)?;
            if !relaxed.is_empty() {
                return Ok(relaxed);
            }
        } else if !hits.is_empty() {
            return Ok(hits);
        }
        self.fuzzy_memory_search(request, limit)
    }

    fn fuzzy_memory_search(&self, request: &SearchRequest, limit: usize) -> Result<Vec<SearchHit>> {
        if request
            .source_kind
            .as_deref()
            .is_some_and(|kind| kind != "memory")
        {
            return Ok(Vec::new());
        }
        let mut statement = self.connection.prepare(
            "SELECT d.id, c.id, d.source_uri, d.source_kind, d.scope, d.title, c.content
             FROM chunks c
             JOIN documents d ON d.id = c.document_id
             JOIN memory_items m ON m.document_id = d.id
             WHERE d.active = 1
               AND d.source_kind = 'memory'
               AND m.superseded_by IS NULL
               AND (m.valid_until_ms IS NULL OR m.valid_until_ms > ?1)
               AND (?2 IS NULL OR d.scope = ?2)
             ORDER BY m.pinned DESC, m.importance DESC,
                      m.last_confirmed_at_ms DESC, c.id
             LIMIT 2000",
        )?;
        let mut hits = statement
            .query_map(params![now_ms(), request.scope.as_deref()], |row| {
                Ok(SearchHit {
                    document_id: row.get(0)?,
                    chunk_id: row.get(1)?,
                    source_uri: row.get(2)?,
                    source_kind: row.get(3)?,
                    scope: row.get(4)?,
                    title: row.get(5)?,
                    content: row.get(6)?,
                    score: 0.0,
                    lexical_rank: None,
                    vector_rank: None,
                    vector_distance: None,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        hits.retain_mut(|hit| {
            if let Some(score) = fuzzy_context_score(&request.query, &hit.content) {
                hit.score = score;
                true
            } else {
                false
            }
        });
        hits.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.chunk_id.cmp(&right.chunk_id))
        });
        hits.truncate(limit);
        for (index, hit) in hits.iter_mut().enumerate() {
            hit.lexical_rank = Some(index + 1);
        }
        Ok(hits)
    }

    fn lexical_search_query(
        &self,
        request: &SearchRequest,
        limit: usize,
        fts_query: &str,
    ) -> Result<Vec<SearchHit>> {
        let mut metadata = self.connection.prepare(
            "SELECT d.id, c.id, d.source_uri, d.source_kind, d.scope, d.title, c.content
             FROM chunks c
             JOIN documents d ON d.id = c.document_id
             WHERE c.id = ?1
               AND d.active = 1
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
               AND (?3 IS NULL OR d.source_kind = ?3)",
        )?;

        let mut candidate_limit = limit.saturating_mul(4).clamp(1, 400);
        let mut active_fallback = false;
        loop {
            let ranked_ids = if active_fallback {
                let mut statement = self.connection.prepare(
                    "SELECT chunk_fts.rowid,
                            bm25(chunk_fts, 4.0, 1.0, 0.0, 0.0) AS lexical_score
                     FROM chunk_fts
                     JOIN chunks c ON c.id = chunk_fts.rowid
                     JOIN documents d ON d.id = c.document_id
                     LEFT JOIN memory_items m ON m.document_id = d.id
                     WHERE chunk_fts MATCH ?1
                       AND d.active = 1
                       AND (
                           m.document_id IS NULL
                           OR (
                               m.superseded_by IS NULL
                               AND (m.valid_until_ms IS NULL OR m.valid_until_ms > ?4)
                           )
                       )
                       AND (?2 IS NULL OR d.scope = ?2)
                       AND (?3 IS NULL OR d.source_kind = ?3)
                     ORDER BY lexical_score, chunk_fts.rowid
                     LIMIT ?5",
                )?;
                statement
                    .query_map(
                        params![
                            fts_query,
                            request.scope.as_deref(),
                            request.source_kind.as_deref(),
                            now_ms(),
                            limit.saturating_mul(4).clamp(1, 400) as i64,
                        ],
                        |row| row.get::<_, i64>(0),
                    )?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            } else {
                let mut statement = self.connection.prepare(
                    "SELECT rowid, bm25(chunk_fts, 4.0, 1.0, 0.0, 0.0) AS lexical_score
                     FROM chunk_fts
                     WHERE chunk_fts MATCH ?1
                       AND (?2 IS NULL OR scope = ?2)
                       AND (?3 IS NULL OR source_kind = ?3)
                     ORDER BY lexical_score, rowid
                     LIMIT ?4",
                )?;
                statement
                    .query_map(
                        params![
                            fts_query,
                            request.scope.as_deref(),
                            request.source_kind.as_deref(),
                            candidate_limit as i64,
                        ],
                        |row| row.get::<_, i64>(0),
                    )?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            };

            let saturated = !active_fallback && ranked_ids.len() == candidate_limit;
            let mut hits = Vec::with_capacity(limit);
            for (rank, chunk_id) in ranked_ids.into_iter().enumerate() {
                let hit = metadata
                    .query_row(
                        params![
                            chunk_id,
                            request.scope.as_deref(),
                            request.source_kind.as_deref(),
                            now_ms(),
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
                                score: 1.0 / (rank as f64 + 1.0),
                                lexical_rank: Some(rank + 1),
                                vector_rank: None,
                                vector_distance: None,
                            })
                        },
                    )
                    .optional()?;
                if let Some(hit) = hit {
                    hits.push(hit);
                    if hits.len() >= limit {
                        break;
                    }
                }
            }
            if hits.len() >= limit || !saturated || active_fallback {
                return Ok(hits);
            }
            if candidate_limit < 6_400 {
                candidate_limit = candidate_limit.saturating_mul(4).min(6_400);
            } else {
                active_fallback = true;
            }
        }
    }

    fn semantic_search(
        &self,
        request: &SearchRequest,
        query_vector: &[f32],
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        let mut sql = String::from(
            "SELECT
                 d.id, c.id, d.source_uri, d.source_kind, d.scope, d.title, c.content,
                 v.distance
             FROM chunk_vectors v
             JOIN chunks c ON c.id = v.rowid
             JOIN documents d ON d.id = c.document_id
             WHERE v.embedding MATCH ?1 AND k = ?2 AND d.active = 1
               AND (
                   NOT EXISTS (
                       SELECT 1 FROM memory_items m
                       WHERE m.document_id = d.id
                   )
                   OR EXISTS (
                       SELECT 1 FROM memory_items m
                       WHERE m.document_id = d.id
                         AND m.superseded_by IS NULL
                         AND (m.valid_until_ms IS NULL OR m.valid_until_ms > ?3)
                   )
               )",
        );
        let candidate_limit = limit.saturating_mul(4).clamp(1, 400) as i64;
        let mut values = vec![
            Value::Blob(vector_to_blob(query_vector)),
            Value::Integer(candidate_limit),
            Value::Integer(now_ms()),
        ];
        if let Some(scope) = request.scope.as_ref() {
            sql.push_str(&format!(" AND v.scope = ?{}", values.len() + 1));
            values.push(Value::Text(scope.clone()));
        }
        if let Some(source_kind) = request.source_kind.as_ref() {
            sql.push_str(&format!(" AND v.source_kind = ?{}", values.len() + 1));
            values.push(Value::Text(source_kind.clone()));
        }
        sql.push_str(" ORDER BY v.distance, c.id");

        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(values), |row| {
            let distance = row.get::<_, f64>(7)?;
            Ok(SearchHit {
                document_id: row.get(0)?,
                chunk_id: row.get(1)?,
                source_uri: row.get(2)?,
                source_kind: row.get(3)?,
                scope: row.get(4)?,
                title: row.get(5)?,
                content: row.get(6)?,
                score: 1.0 / (1.0 + distance.max(0.0)),
                lexical_rank: None,
                vector_rank: None,
                vector_distance: Some(distance),
            })
        })?;
        let mut hits = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        hits.truncate(limit);
        for (index, hit) in hits.iter_mut().enumerate() {
            hit.vector_rank = Some(index + 1);
        }
        Ok(hits)
    }

    pub fn health(&self) -> Result<HealthReport> {
        let integrity = self
            .connection
            .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))?;
        let sqlite_version = self
            .connection
            .query_row("SELECT sqlite_version()", [], |row| row.get::<_, String>(0))?;
        let vector_version = self
            .connection
            .query_row("SELECT vec_version()", [], |row| row.get::<_, String>(0))?;
        let schema_version = self
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?;
        let foreign_key_violations = {
            let mut statement = self.connection.prepare("PRAGMA foreign_key_check")?;
            let mut rows = statement.query([])?;
            let mut count = 0usize;
            while rows.next()?.is_some() {
                count += 1;
            }
            count
        };
        let invalid_heads = self.connection.query_row(
            "SELECT count(*)
             FROM memory_heads h
             LEFT JOIN memory_items m ON m.document_id = h.document_id
             WHERE m.document_id IS NULL
                OR m.canonical_key <> h.canonical_key
                OR m.superseded_by IS NOT NULL",
            [],
            |row| row.get::<_, i64>(0),
        )? as usize;
        let duplicate_active_keys = self.connection.query_row(
            "SELECT count(*) FROM (
                 SELECT canonical_key
                 FROM memory_items
                 WHERE canonical_key IS NOT NULL AND superseded_by IS NULL
                 GROUP BY canonical_key
                 HAVING count(*) > 1
             )",
            [],
            |row| row.get::<_, i64>(0),
        )? as usize;
        let missing_heads = self.connection.query_row(
            "SELECT count(*)
             FROM memory_items m
             LEFT JOIN memory_heads h
               ON h.canonical_key = m.canonical_key AND h.document_id = m.document_id
             WHERE m.canonical_key IS NOT NULL
               AND m.superseded_by IS NULL
               AND h.document_id IS NULL",
            [],
            |row| row.get::<_, i64>(0),
        )? as usize;
        let supersession_cycles = self.connection.query_row(
            "WITH RECURSIVE supersession(origin, current) AS (
                 SELECT document_id, superseded_by
                 FROM memory_items
                 WHERE superseded_by IS NOT NULL
                 UNION
                 SELECT s.origin, m.superseded_by
                 FROM supersession s
                 JOIN memory_items m ON m.document_id = s.current
                 WHERE m.superseded_by IS NOT NULL
             )
             SELECT count(DISTINCT origin)
             FROM supersession
             WHERE origin = current",
            [],
            |row| row.get::<_, i64>(0),
        )? as usize;
        let invalid_citations = self.connection.query_row(
            "SELECT count(*)
             FROM memory_citations c
             JOIN evidence_sessions e ON e.id = c.evidence_session_id
             JOIN documents d ON d.id = c.evidence_document_id
             WHERE c.evidence_document_id <> e.document_id
                OR c.end_byte > length(CAST(d.body AS BLOB))
                OR CAST(substr(
                       CAST(d.body AS BLOB),
                       c.start_byte + 1,
                       c.end_byte - c.start_byte
                   ) AS TEXT) <> c.quote",
            [],
            |row| row.get::<_, i64>(0),
        )? as usize;
        let now = now_ms();
        let unexpected_fts_rows = self.connection.query_row(
            "SELECT count(*)
             FROM chunk_fts
             LEFT JOIN chunks c ON c.id = chunk_fts.rowid
             LEFT JOIN documents d ON d.id = c.document_id
             LEFT JOIN memory_items m ON m.document_id = d.id
             WHERE c.id IS NULL
                OR d.active <> 1
                OR (
                    m.document_id IS NOT NULL
                    AND (
                        m.superseded_by IS NOT NULL
                        OR (m.valid_until_ms IS NOT NULL AND m.valid_until_ms <= ?1)
                    )
                )",
            [now],
            |row| row.get::<_, i64>(0),
        )? as usize;
        let missing_fts_rows = self.connection.query_row(
            "SELECT count(*)
             FROM chunks c
             JOIN documents d ON d.id = c.document_id
             LEFT JOIN memory_items m ON m.document_id = d.id
             LEFT JOIN chunk_fts ON chunk_fts.rowid = c.id
             WHERE d.active = 1
               AND (
                   m.document_id IS NULL
                   OR (
                       m.superseded_by IS NULL
                       AND (m.valid_until_ms IS NULL OR m.valid_until_ms > ?1)
                   )
               )
               AND chunk_fts.rowid IS NULL",
            [now],
            |row| row.get::<_, i64>(0),
        )? as usize;
        let fts_violations = unexpected_fts_rows + missing_fts_rows;
        let orphan_vectors = self.connection.query_row(
            "SELECT count(*)
             FROM chunk_vectors v
             LEFT JOIN chunks c ON c.id = v.rowid
             WHERE c.id IS NULL OR c.embedding_model IS NULL",
            [],
            |row| row.get::<_, i64>(0),
        )? as usize;
        let missing_vectors = self.connection.query_row(
            "SELECT count(*)
             FROM chunks c
             WHERE c.embedding_model IS NOT NULL
               AND NOT EXISTS (
                   SELECT 1 FROM chunk_vectors v WHERE v.rowid = c.id
               )",
            [],
            |row| row.get::<_, i64>(0),
        )? as usize;
        let mixed_vector_models = self.connection.query_row(
            "SELECT count(*)
             FROM chunks c
             WHERE c.embedding_model IS NOT NULL
               AND c.embedding_model <> COALESCE(
                   (SELECT value FROM metadata WHERE key = 'embedding_model'),
                   ''
               )",
            [],
            |row| row.get::<_, i64>(0),
        )? as usize;
        let vector_violations = orphan_vectors + missing_vectors + mixed_vector_models;
        let queue_violations = self.connection.query_row(
            "SELECT count(*)
             FROM embedding_queue
             WHERE (claimed_by IS NULL) <> (lease_until_ms IS NULL)
                OR attempts < 0
                OR priority < 0",
            [],
            |row| row.get::<_, i64>(0),
        )? as usize;
        let memory_violations =
            invalid_heads + duplicate_active_keys + missing_heads + supersession_cycles;
        let citation_violations = invalid_citations;
        let logical_violations = memory_violations
            + citation_violations
            + fts_violations
            + vector_violations
            + queue_violations;
        let leased_embeddings = self.connection.query_row(
            "SELECT count(*) FROM embedding_queue WHERE lease_until_ms > ?1",
            [now],
            |row| row.get::<_, i64>(0),
        )? as usize;
        let failed_embeddings = self.connection.query_row(
            "SELECT count(*) FROM embedding_queue WHERE last_error IS NOT NULL",
            [],
            |row| row.get::<_, i64>(0),
        )? as usize;
        let dead_embeddings = self.connection.query_row(
            "SELECT count(*) FROM embedding_queue
             WHERE attempts >= ?1 AND last_error IS NOT NULL",
            [MAX_EMBED_ATTEMPTS],
            |row| row.get::<_, i64>(0),
        )? as usize;
        let retrying_embeddings = self.connection.query_row(
            "SELECT count(*) FROM embedding_queue
             WHERE attempts < ?1 AND last_error IS NOT NULL",
            [MAX_EMBED_ATTEMPTS],
            |row| row.get::<_, i64>(0),
        )? as usize;
        let active_memory_chunks = self.connection.query_row(
            "SELECT count(*)
             FROM chunks c
             JOIN documents d ON d.id = c.document_id
             JOIN memory_items m ON m.document_id = d.id
             WHERE d.active = 1
               AND d.source_kind = 'memory'
               AND m.superseded_by IS NULL
               AND (m.valid_until_ms IS NULL OR m.valid_until_ms > ?1)",
            [now],
            |row| row.get::<_, i64>(0),
        )? as usize;
        let active_memory_vectors = self.connection.query_row(
            "SELECT count(*)
             FROM chunks c
             JOIN documents d ON d.id = c.document_id
             JOIN memory_items m ON m.document_id = d.id
             JOIN chunk_vectors v ON v.rowid = c.id
             WHERE d.active = 1
               AND d.source_kind = 'memory'
               AND m.superseded_by IS NULL
               AND (m.valid_until_ms IS NULL OR m.valid_until_ms > ?1)",
            [now],
            |row| row.get::<_, i64>(0),
        )? as usize;
        let reference_chunks = self.connection.query_row(
            "SELECT count(*)
             FROM chunks c
             JOIN documents d ON d.id = c.document_id
             WHERE d.active = 1 AND d.source_kind NOT IN ('memory', 'evidence')",
            [],
            |row| row.get::<_, i64>(0),
        )? as usize;
        let reference_vectors = self.connection.query_row(
            "SELECT count(*)
             FROM chunks c
             JOIN documents d ON d.id = c.document_id
             JOIN chunk_vectors v ON v.rowid = c.id
             WHERE d.active = 1 AND d.source_kind NOT IN ('memory', 'evidence')",
            [],
            |row| row.get::<_, i64>(0),
        )? as usize;
        let evidence_vectors = self.connection.query_row(
            "SELECT count(*)
             FROM chunks c
             JOIN documents d ON d.id = c.document_id
             JOIN chunk_vectors v ON v.rowid = c.id
             WHERE d.source_kind = 'evidence'",
            [],
            |row| row.get::<_, i64>(0),
        )? as usize;
        Ok(HealthReport {
            ok: integrity == "ok"
                && schema_version == SCHEMA_VERSION
                && foreign_key_violations == 0
                && logical_violations == 0
                && dead_embeddings == 0,
            database_path: self.path.clone(),
            sqlite_version,
            vector_version,
            schema_version,
            embedding_dimensions: self.embedding_dimensions,
            embedding_model: self.metadata_value("embedding_model")?,
            documents: self.count("documents WHERE active = 1")?,
            chunks: self.count("chunks")?,
            vectors: self.count("chunk_vectors")?,
            pending_embeddings: self.count("embedding_queue")?,
            leased_embeddings,
            failed_embeddings,
            retrying_embeddings,
            dead_embeddings,
            active_memory_chunks,
            active_memory_vectors,
            reference_chunks,
            reference_vectors,
            evidence_vectors,
            evidence_sessions: self.count("evidence_sessions")?,
            active_memories: self.connection.query_row(
                "SELECT count(*)
                 FROM memory_items
                 WHERE superseded_by IS NULL
                   AND (valid_until_ms IS NULL OR valid_until_ms > ?1)",
                [now],
                |row| row.get::<_, i64>(0),
            )? as usize,
            citations: self.count("memory_citations")?,
            foreign_key_violations,
            memory_violations,
            citation_violations,
            fts_violations,
            vector_violations,
            queue_violations,
            logical_violations,
            integrity,
        })
    }

    pub fn backup_to(&self, destination: &Path) -> Result<()> {
        if destination.exists() {
            anyhow::bail!(
                "backup destination already exists: {}",
                destination.display()
            );
        }
        if let Some(parent) = destination.parent() {
            create_private_dir_all(parent)?;
        }
        let mut destination_connection = Connection::open(destination)
            .with_context(|| format!("failed to create backup {}", destination.display()))?;
        set_private_file(destination)?;
        let backup = Backup::new(&self.connection, &mut destination_connection)?;
        backup.run_to_completion(128, Duration::from_millis(10), None)?;
        set_private_file(destination)?;
        Ok(())
    }

    pub fn rebuild_fts(&mut self) -> Result<usize> {
        let transaction = self.connection.transaction()?;
        transaction.execute("DELETE FROM chunk_fts", [])?;
        let inserted = transaction.execute(
            "INSERT INTO chunk_fts(rowid, title, content, source_uri, scope, source_kind)
             SELECT c.id, d.title, c.content, d.source_uri, d.scope, d.source_kind
             FROM chunks c JOIN documents d ON d.id = c.document_id
             WHERE d.active = 1
               AND (
                   NOT EXISTS (
                       SELECT 1 FROM memory_items m
                       WHERE m.document_id = d.id
                   )
                   OR EXISTS (
                       SELECT 1 FROM memory_items m
                       WHERE m.document_id = d.id
                         AND m.superseded_by IS NULL
                         AND (m.valid_until_ms IS NULL OR m.valid_until_ms > ?1)
                   )
               )",
            [now_ms()],
        )?;
        transaction.commit()?;
        Ok(inserted)
    }

    pub fn requeue_embeddings(&mut self) -> Result<usize> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let active_leases = transaction.query_row(
            "SELECT count(*) FROM embedding_queue WHERE lease_until_ms > ?1",
            [now_ms()],
            |row| row.get::<_, i64>(0),
        )?;
        if active_leases > 0 {
            anyhow::bail!(
                "cannot requeue embeddings while {active_leases} embedding jobs are leased"
            );
        }
        transaction.execute("DELETE FROM chunk_vectors", [])?;
        transaction.execute(
            "UPDATE chunks SET embedding_model = NULL, embedded_at_ms = NULL",
            [],
        )?;
        transaction.execute("DELETE FROM embedding_queue", [])?;
        transaction.execute("DELETE FROM metadata WHERE key = 'embedding_model'", [])?;
        let queued = transaction.execute(
            "INSERT INTO embedding_queue(chunk_id, queued_at_ms, priority)
             SELECT c.id, ?1,
                    CASE WHEN d.source_kind = 'memory' THEN 100 ELSE 50 END
             FROM chunks c
             JOIN documents d ON d.id = c.document_id
             WHERE d.active = 1
               AND d.source_kind <> 'evidence'
               AND (
                   NOT EXISTS (
                       SELECT 1 FROM memory_items m
                       WHERE m.document_id = d.id
                   )
                   OR EXISTS (
                       SELECT 1 FROM memory_items m
                       WHERE m.document_id = d.id
                         AND m.superseded_by IS NULL
                         AND (m.valid_until_ms IS NULL OR m.valid_until_ms > ?1)
                   )
               )
             ON CONFLICT(chunk_id) DO UPDATE SET
                 queued_at_ms = excluded.queued_at_ms,
                 attempts = 0,
                 last_error = NULL,
                 claimed_by = NULL,
                 lease_until_ms = NULL,
                 last_attempt_at_ms = NULL,
                 priority = excluded.priority,
                 next_attempt_at_ms = NULL",
            [now_ms()],
        )?;
        transaction.commit()?;
        Ok(queued)
    }

    pub fn export_memories(&self, destination: &Path) -> Result<usize> {
        if destination.exists() {
            anyhow::bail!(
                "export destination already exists: {}",
                destination.display()
            );
        }
        if let Some(parent) = destination.parent() {
            create_private_dir_all(parent)?;
        }
        let mut statement = self.connection.prepare(
            "SELECT d.id, d.title, d.body, d.scope, m.memory_kind, m.importance,
                    m.confidence, m.pinned, d.modified_at_ms
             FROM memory_items m
             JOIN documents d ON d.id = m.document_id
             WHERE d.active = 1
               AND m.superseded_by IS NULL
               AND (m.valid_until_ms IS NULL OR m.valid_until_ms > ?1)
             ORDER BY m.pinned DESC, m.importance DESC, d.modified_at_ms DESC, d.id",
        )?;
        let memories = statement
            .query_map([now_ms()], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, f64>(5)?,
                    row.get::<_, f64>(6)?,
                    row.get::<_, bool>(7)?,
                    row.get::<_, i64>(8)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut output = String::from(
            "---\nmoon_export: 1\ngenerated: true\ncanonical_source: moon.sqlite\n---\n\n# Moon Memory Export\n\n",
        );
        for (id, title, body, scope, kind, importance, confidence, pinned, modified) in &memories {
            let heading = title.as_deref().unwrap_or("Untitled memory");
            output.push_str(&format!(
                "## {heading}\n\n- id: {id}\n- kind: {kind}\n- scope: {scope}\n- importance: {importance:.3}\n- confidence: {confidence:.3}\n- pinned: {pinned}\n- modified_at_ms: {modified}\n\n{}\n\n",
                body.trim()
            ));
        }
        fs::write(destination, output)
            .with_context(|| format!("failed to write {}", destination.display()))?;
        set_private_file(destination)?;
        Ok(memories.len())
    }

    pub fn set_state(&self, key: &str, value: &serde_json::Value) -> Result<()> {
        if key.trim().is_empty() {
            anyhow::bail!("state key must not be empty");
        }
        self.connection.execute(
            "INSERT INTO runtime_state(key, value_json, updated_at_ms)
             VALUES(?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE
             SET value_json = excluded.value_json, updated_at_ms = excluded.updated_at_ms",
            params![key, serde_json::to_string(value)?, now_ms()],
        )?;
        Ok(())
    }

    pub fn get_state(&self, key: &str) -> Result<Option<serde_json::Value>> {
        self.connection
            .query_row(
                "SELECT value_json FROM runtime_state WHERE key = ?1",
                [key],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|raw| serde_json::from_str(&raw).context("stored state contains invalid JSON"))
            .transpose()
    }

    fn count(&self, from_clause: &str) -> Result<usize> {
        let allowed = [
            "documents WHERE active = 1",
            "chunks",
            "chunk_vectors",
            "embedding_queue",
            "evidence_sessions",
            "memory_items WHERE superseded_by IS NULL",
            "memory_citations",
        ];
        if !allowed.contains(&from_clause) {
            anyhow::bail!("unsupported count target");
        }
        let sql = format!("SELECT count(*) FROM {from_clause}");
        Ok(self
            .connection
            .query_row(&sql, [], |row| row.get::<_, i64>(0))? as usize)
    }

    fn metadata_value(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .connection
            .query_row("SELECT value FROM metadata WHERE key = ?1", [key], |row| {
                row.get::<_, String>(0)
            })
            .optional()?)
    }
}

pub(crate) fn prepare_ingest(document: IngestDocument) -> Result<PreparedIngest> {
    validate_document(&document)?;
    let content_hash = sha256_hex(&document.content);
    let chunks = chunk_text(
        &document.content,
        DEFAULT_CHUNK_CHARS,
        DEFAULT_CHUNK_OVERLAP_CHARS,
    );
    if chunks.is_empty() {
        anyhow::bail!("document content is empty");
    }
    serde_json::from_str::<serde_json::Value>(&document.metadata_json)
        .context("metadata_json must contain valid JSON")?;
    Ok(PreparedIngest {
        document,
        content_hash,
        chunks,
    })
}

pub(crate) fn ingest_prepared(
    transaction: &Transaction<'_>,
    prepared: PreparedIngest,
) -> Result<IngestOutcome> {
    let PreparedIngest {
        document,
        content_hash,
        chunks,
    } = prepared;
    let existing = transaction
        .query_row(
            "SELECT id, content_hash, active, source_kind, scope, title
             FROM documents WHERE source_uri = ?1",
            [&document.source_uri],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, bool>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            },
        )
        .optional()?;

    if let Some((document_id, existing_hash, true, source_kind, scope, title)) = existing.as_ref()
        && existing_hash == &content_hash
        && source_kind == &document.source_kind
        && scope == &document.scope
        && title == &document.title
    {
        let chunk_count = transaction.query_row(
            "SELECT count(*) FROM chunks WHERE document_id = ?1",
            [document_id],
            |row| row.get::<_, i64>(0),
        )? as usize;
        return Ok(IngestOutcome {
            document_id: *document_id,
            source_uri: document.source_uri,
            chunks: chunk_count,
            changed: false,
        });
    }

    let document_id = if let Some((document_id, _, _, _, _, _)) = existing {
        let mut statement = transaction.prepare("SELECT id FROM chunks WHERE document_id = ?1")?;
        let old_chunk_ids = statement
            .query_map([document_id], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        for chunk_id in old_chunk_ids {
            transaction.execute("DELETE FROM chunk_vectors WHERE rowid = ?1", [chunk_id])?;
            transaction.execute("DELETE FROM chunk_fts WHERE rowid = ?1", [chunk_id])?;
        }
        transaction.execute("DELETE FROM chunks WHERE document_id = ?1", [document_id])?;
        transaction.execute(
            "UPDATE documents
             SET source_kind = ?2, scope = ?3, title = ?4, content_hash = ?5,
                 modified_at_ms = ?6, indexed_at_ms = ?7, active = 1,
                 metadata_json = ?8, body = ?9
             WHERE id = ?1",
            params![
                document_id,
                document.source_kind,
                document.scope,
                document.title,
                content_hash,
                document.modified_at_ms,
                now_ms(),
                document.metadata_json,
                document.content,
            ],
        )?;
        document_id
    } else {
        transaction.execute(
            "INSERT INTO documents(
                 source_uri, source_kind, scope, title, content_hash,
                 modified_at_ms, indexed_at_ms, metadata_json, body
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                document.source_uri,
                document.source_kind,
                document.scope,
                document.title,
                content_hash,
                document.modified_at_ms,
                now_ms(),
                document.metadata_json,
                document.content,
            ],
        )?;
        transaction.last_insert_rowid()
    };

    for chunk in &chunks {
        transaction.execute(
            "INSERT INTO chunks(
                 document_id, ordinal, content, content_hash, start_byte, end_byte
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                document_id,
                chunk.ordinal as i64,
                chunk.content,
                chunk.content_hash,
                chunk.start_byte as i64,
                chunk.end_byte as i64,
            ],
        )?;
        let chunk_id = transaction.last_insert_rowid();
        transaction.execute(
            "INSERT INTO chunk_fts(rowid, title, content, source_uri, scope, source_kind)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                chunk_id,
                document.title,
                chunk.content,
                document.source_uri,
                document.scope,
                document.source_kind,
            ],
        )?;
        if let Some(priority) = embedding_priority(&document.source_kind) {
            transaction.execute(
                "INSERT INTO embedding_queue(chunk_id, queued_at_ms, priority)
                 VALUES(?1, ?2, ?3)",
                params![chunk_id, now_ms(), priority],
            )?;
        }
    }

    Ok(IngestOutcome {
        document_id,
        source_uri: document.source_uri,
        chunks: chunks.len(),
        changed: true,
    })
}

fn embedding_priority(source_kind: &str) -> Option<i64> {
    match source_kind {
        "evidence" => None,
        "memory" => Some(100),
        _ => Some(50),
    }
}

pub(crate) fn random_nonce(connection: &Connection) -> Result<String> {
    Ok(
        connection.query_row("SELECT lower(hex(randomblob(16)))", [], |row| {
            row.get::<_, String>(0)
        })?,
    )
}

fn validate_vectors(
    vectors: Vec<Vec<f32>>,
    expected_count: usize,
    expected_dimensions: usize,
) -> Result<Vec<Vec<f32>>> {
    if vectors.len() != expected_count {
        anyhow::bail!(
            "provider returned {} vectors for {expected_count} chunks",
            vectors.len()
        );
    }
    for vector in &vectors {
        if vector.len() != expected_dimensions {
            anyhow::bail!(
                "provider returned vector with {} dimensions; expected {expected_dimensions}",
                vector.len()
            );
        }
        if vector.iter().any(|value| !value.is_finite()) {
            anyhow::bail!("provider returned a vector containing a non-finite value");
        }
    }
    Ok(vectors)
}

fn sanitized_error(error: &anyhow::Error) -> String {
    let redacted = redact_text(&format!("{error:#}")).value;
    let mut message = redacted.chars().take(500).collect::<String>();
    if redacted.chars().count() > 500 {
        message.push('…');
    }
    message
}

fn create_private_dir_all(path: &Path) -> Result<()> {
    let mut missing = Vec::new();
    let mut cursor = Some(path);
    while let Some(current) = cursor {
        if current.exists() {
            break;
        }
        missing.push(current.to_path_buf());
        cursor = current.parent();
    }
    fs::create_dir_all(path).with_context(|| format!("failed to create {}", path.display()))?;
    for directory in missing.into_iter().rev() {
        set_private_dir(&directory)?;
    }
    Ok(())
}

#[cfg(unix)]
fn set_private_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("failed to protect directory {}", path.display()))
}

#[cfg(not(unix))]
fn set_private_dir(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("failed to protect file {}", path.display()))
}

#[cfg(not(unix))]
fn set_private_file(_path: &Path) -> Result<()> {
    Ok(())
}

fn set_private_sqlite_sidecars(path: &Path) -> Result<()> {
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = path.as_os_str().to_os_string();
        sidecar.push(suffix);
        let sidecar = PathBuf::from(sidecar);
        if sidecar.exists() {
            set_private_file(&sidecar)?;
        }
    }
    Ok(())
}

fn register_vector_extension() {
    REGISTER_VECTOR_EXTENSION.call_once(|| unsafe {
        // sqlite-vec is statically linked. Registering it as an automatic
        // extension makes it available to every SQLite connection in-process.
        type ExtensionInit = unsafe extern "C" fn(
            *mut rusqlite::ffi::sqlite3,
            *mut *mut std::ffi::c_char,
            *const rusqlite::ffi::sqlite3_api_routines,
        ) -> std::ffi::c_int;
        rusqlite::ffi::sqlite3_auto_extension(Some(
            std::mem::transmute::<*const (), ExtensionInit>(sqlite3_vec_init as *const ()),
        ));
    });
}

fn validate_dimensions(dimensions: usize) -> Result<()> {
    if !(8..=8_192).contains(&dimensions) {
        anyhow::bail!("embedding dimensions must be between 8 and 8192");
    }
    Ok(())
}

fn validate_document(document: &IngestDocument) -> Result<()> {
    for (name, value) in [
        ("source_uri", document.source_uri.as_str()),
        ("source_kind", document.source_kind.as_str()),
        ("scope", document.scope.as_str()),
    ] {
        if value.trim().is_empty() {
            anyhow::bail!("{name} must not be empty");
        }
        if value.len() > 2_048 {
            anyhow::bail!("{name} is too long");
        }
    }
    if document.content.trim().is_empty() {
        anyhow::bail!("content must not be empty");
    }
    if document.content.len() > MAX_DOCUMENT_BYTES {
        anyhow::bail!(
            "content exceeds the maximum size of {} bytes",
            MAX_DOCUMENT_BYTES
        );
    }
    if document
        .title
        .as_ref()
        .is_some_and(|title| title.len() > MAX_TITLE_BYTES)
    {
        anyhow::bail!("title exceeds the maximum size of {MAX_TITLE_BYTES} bytes");
    }
    if document.metadata_json.len() > MAX_METADATA_BYTES {
        anyhow::bail!("metadata_json exceeds the maximum size of {MAX_METADATA_BYTES} bytes");
    }
    Ok(())
}

fn validate_unit_interval(name: &str, value: f64) -> Result<()> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        anyhow::bail!("{name} must be between 0 and 1");
    }
    Ok(())
}

fn safe_fts_queries(query: &str) -> Result<(String, String)> {
    let mut terms = query
        .split(|character: char| {
            !character.is_alphanumeric() && character != '_' && character != '-'
        })
        .map(|term| term.trim_matches('-'))
        .filter(|term| term.chars().count() >= 2)
        .map(|term| term.to_ascii_lowercase())
        .collect::<Vec<_>>();
    terms.sort();
    terms.dedup();
    terms.truncate(24);
    if terms.is_empty() {
        anyhow::bail!("search query contains no searchable terms");
    }
    let quoted = terms
        .into_iter()
        .map(|term| format!("\"{term}\""))
        .collect::<Vec<_>>();
    Ok((quoted.join(" AND "), quoted.join(" OR ")))
}

fn fuse_rankings(
    lexical: Vec<SearchHit>,
    semantic: Vec<SearchHit>,
    limit: usize,
) -> Vec<SearchHit> {
    let mut fused = BTreeMap::<i64, SearchHit>::new();
    for (index, mut hit) in lexical.into_iter().enumerate() {
        hit.lexical_rank = Some(index + 1);
        hit.score = 1.0 / (RRF_K + index as f64 + 1.0);
        fused.insert(hit.chunk_id, hit);
    }
    for (index, semantic_hit) in semantic.into_iter().enumerate() {
        let contribution = 1.0 / (RRF_K + index as f64 + 1.0);
        match fused.get_mut(&semantic_hit.chunk_id) {
            Some(existing) => {
                existing.score += contribution;
                existing.vector_rank = Some(index + 1);
                existing.vector_distance = semantic_hit.vector_distance;
            }
            None => {
                let mut hit = semantic_hit;
                hit.score = contribution;
                hit.vector_rank = Some(index + 1);
                fused.insert(hit.chunk_id, hit);
            }
        }
    }
    let mut results = fused.into_values().collect::<Vec<_>>();
    results.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.source_uri.cmp(&right.source_uri))
            .then_with(|| left.chunk_id.cmp(&right.chunk_id))
    });
    results.truncate(limit);
    results
}

pub(crate) fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

#[cfg(test)]
mod tests {
    use super::safe_fts_queries;

    #[test]
    fn fts_query_does_not_pass_operators_through() {
        assert_eq!(
            safe_fts_queries(r#"Moon OR "danger" -query"#).expect("query"),
            (
                "\"danger\" AND \"moon\" AND \"or\" AND \"query\"".to_string(),
                "\"danger\" OR \"moon\" OR \"or\" OR \"query\"".to_string()
            )
        );
    }
}
