//! Explicit, inert source capture. No network, file traversal, model calls or
//! indexing of raw sources occurs here. Eligible captures queue reviewable work.
//! Citations always address immutable sanitised UTF-8.
use crate::Store;
use crate::chunking::sha256_hex;
use crate::redaction::redact_text;
use anyhow::{Result, bail, ensure};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const SOURCE_SCHEMA_VERSION: u32 = 1;
pub const MAX_SOURCE_BYTES: usize = 1024 * 1024;
const MAX_CITATION_BYTES: usize = 8192;
const EXTRACTION_VERSION: &str = "moon-text-v1";

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    ExternalDocument,
    UserStatement,
    Reference,
    GeneratedAnalysis,
}

#[derive(Debug, Default, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceFormat {
    #[default]
    Text,
    Markdown,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceInput {
    pub schema_version: u32,
    pub source_key: String,
    pub kind: SourceKind,
    pub content: String,
    #[serde(default)]
    pub format: SourceFormat,
    pub origin: Option<String>,
    pub title: Option<String>,
    pub author: Option<String>,
    pub published_at_ms: Option<i64>,
}

impl SourceInput {
    /// Also usable before opening storage, so a malformed request cannot cause
    /// a database creation or migration. Errors never interpolate source data.
    pub fn validate(&self, scope: &str, expected_revision: Option<&str>) -> Result<()> {
        ensure!(
            self.schema_version == SOURCE_SCHEMA_VERSION,
            "unsupported source schema version"
        );
        validate_identity(scope, false)?;
        validate_identity(&self.source_key, true)?;
        if let Some(id) = expected_revision {
            validate_revision_id(id)?;
        }
        validate_text(&self.content, MAX_SOURCE_BYTES, true)?;
        validate_text(&redact_text(&self.content).value, MAX_SOURCE_BYTES, true)?;
        for text in [&self.title, &self.author].into_iter().flatten() {
            validate_text(text, 4096, false)?;
            validate_text(&redact_text(text).value, 4096, false)?;
        }
        if let Some(origin) = &self.origin {
            validate_origin(origin)?;
        }
        ensure!(
            self.published_at_ms.is_none_or(|time| time >= 0),
            "invalid source publication time"
        );
        Ok(())
    }
}

/// Only the sanitised material representation participates in identity checks.
/// Capture time and redaction counts are observations, not new source versions.
struct PreparedSource {
    kind: String,
    format: String,
    eligibility: &'static str,
    origin: Option<String>,
    title: Option<String>,
    author: Option<String>,
    content: String,
    content_hash: String,
    revision_hash: String,
    published_at_ms: Option<i64>,
    redactions: usize,
}

impl PreparedSource {
    fn new(input: SourceInput, allow_learning: bool) -> Result<Self> {
        let mut redactions = 0;
        let mut sanitise = |text: String| {
            let result = redact_text(&text);
            redactions += result.count;
            result.value
        };
        let content = sanitise(input.content);
        // Redaction markers can expand very short secrets. Enforce the bound
        // again on what actually reaches SQLite, not only on the incoming bytes.
        validate_text(&content, MAX_SOURCE_BYTES, true)?;
        let title = input.title.map(&mut sanitise);
        let author = input.author.map(&mut sanitise);
        for field in [&title, &author].into_iter().flatten() {
            validate_text(field, 4096, false)?;
        }
        let kind = enum_string(input.kind);
        let format = enum_string(input.format);
        let eligibility = if allow_learning {
            "eligible"
        } else {
            "reference_only"
        };
        let content_hash = sha256_hex(&content);
        let revision_hash = revision_hash(
            &kind,
            &format,
            eligibility,
            input.origin.as_deref(),
            title.as_deref(),
            author.as_deref(),
            &content_hash,
            input.published_at_ms,
        );
        Ok(Self {
            kind,
            format,
            eligibility,
            origin: input.origin,
            title,
            author,
            content,
            content_hash,
            revision_hash,
            published_at_ms: input.published_at_ms,
            redactions,
        })
    }
}

impl Store {
    pub fn source_import(
        &mut self,
        scope: &str,
        input: SourceInput,
        expected_revision: Option<&str>,
        allow_learning: bool,
    ) -> Result<Value> {
        input.validate(scope, expected_revision)?;
        let key = input.source_key.clone();
        let prepared = PreparedSource::new(input, allow_learning)?;
        // Serialise read/compare/append across independent CLI processes. A stale
        // producer cannot overwrite a newer capture after waiting for the lock.
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure!(
            !tx.query_row("SELECT EXISTS(SELECT 1 FROM sources s JOIN source_lifecycle l ON l.source_id=s.source_id WHERE s.scope=?1 AND s.source_key=?2 AND l.status='deleted')", params![scope,key], |row| row.get::<_,bool>(0))?,
            "deleted source identities cannot be reimported"
        );
        let existing = find_revision(&tx, scope, &key, None)?;
        if let Some(current) = &existing {
            if current.revision_hash == prepared.revision_hash {
                let mut response = current.summary();
                response["status"] = json!("unchanged");
                tx.commit()?;
                return Ok(response);
            }
            ensure!(
                expected_revision == Some(current.revision_id.as_str()),
                "source revision conflict: provide the current expected revision"
            );
        } else {
            ensure!(
                expected_revision.is_none(),
                "source revision conflict: source does not exist"
            );
        }
        let now = Utc::now().timestamp_millis();
        let (source_id, revision) = if let Some(current) = existing {
            (
                current.source_id,
                current
                    .revision
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("source revision limit reached"))?,
            )
        } else {
            let id = format!("src_{:032x}", rand::random::<u128>());
            tx.execute("INSERT INTO sources(source_id, scope, source_key, created_at_ms) VALUES (?1, ?2, ?3, ?4)", params![id, scope, key, now])?;
            (id, 1)
        };
        let revision_id = format!("sr_{:032x}", rand::random::<u128>());
        tx.execute(
            "INSERT INTO source_revisions(revision_id, source_id, revision, kind, format, learning_eligibility, origin, title, author, content, content_hash, revision_hash, published_at_ms, captured_at_ms, redactions, extraction_version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![revision_id, source_id, revision, prepared.kind, prepared.format, prepared.eligibility, prepared.origin, prepared.title, prepared.author, prepared.content, prepared.content_hash, prepared.revision_hash, prepared.published_at_ms, now, prepared.redactions as i64, EXTRACTION_VERSION],
        )?;
        crate::source_learning::source_revision_captured(&tx, &source_id)?;
        let saved = find_revision(&tx, scope, &key, Some(&revision_id))?
            .ok_or_else(|| anyhow::anyhow!("source transaction invariant failed"))?;
        let mut response = saved.summary();
        response["status"] = json!("created");
        tx.commit()?;
        Ok(response)
    }

    pub fn source_get(&self, scope: &str, key: &str, revision: Option<&str>) -> Result<Value> {
        validate_lookup(scope, key, revision)?;
        let stored = find_revision(&self.connection, scope, key, revision)?
            .ok_or_else(|| anyhow::anyhow!("source revision not found"))?;
        let mut result = stored.summary();
        result["content"] = json!(stored.content);
        Ok(result)
    }

    pub fn source_list(&self, scope: &str, limit: usize, after_key: Option<&str>) -> Result<Value> {
        validate_identity(scope, false)?;
        validate_limit(limit)?;
        if let Some(key) = after_key {
            validate_identity(key, true)?;
        }
        let sql = format!("{}
            WHERE s.scope = ?1 AND (?2 IS NULL OR s.source_key > ?2)
              AND r.revision = (SELECT max(h.revision) FROM source_revisions h WHERE h.source_id = s.source_id)
            ORDER BY s.source_key LIMIT ?3", select_summary());
        let mut statement = self.connection.prepare(&sql)?;
        let mut records = statement
            .query_map(
                params![scope, after_key, (limit + 1) as i64],
                StoredRevision::read,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let more = records.len() > limit;
        records.truncate(limit);
        let cursor = more.then(|| records.last().expect("nonzero limit").source_key.clone());
        Ok(
            json!({"schema_version": SOURCE_SCHEMA_VERSION, "sources": records.iter().map(StoredRevision::summary).collect::<Vec<_>>(), "next_after_key": cursor}),
        )
    }

    pub fn source_history(
        &self,
        scope: &str,
        key: &str,
        limit: usize,
        before_revision: Option<i64>,
    ) -> Result<Value> {
        validate_lookup(scope, key, None)?;
        validate_limit(limit)?;
        ensure!(
            before_revision.is_none_or(|value| value > 0),
            "invalid source history cursor"
        );
        ensure!(
            find_revision(&self.connection, scope, key, None)?.is_some(),
            "source revision not found"
        );
        let sql = format!(
            "{} WHERE s.scope = ?1 AND s.source_key = ?2
            AND (?3 IS NULL OR r.revision < ?3) ORDER BY r.revision DESC LIMIT ?4",
            select_summary()
        );
        let mut statement = self.connection.prepare(&sql)?;
        let mut records = statement
            .query_map(
                params![scope, key, before_revision, (limit + 1) as i64],
                StoredRevision::read,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let more = records.len() > limit;
        records.truncate(limit);
        let cursor = more.then(|| records.last().expect("nonzero limit").revision);
        Ok(
            json!({"schema_version": SOURCE_SCHEMA_VERSION, "revisions": records.iter().map(StoredRevision::summary).collect::<Vec<_>>(), "next_before_revision": cursor}),
        )
    }

    pub fn source_cite(
        &self,
        scope: &str,
        key: &str,
        revision: &str,
        start_byte: usize,
        end_byte: usize,
    ) -> Result<Value> {
        validate_lookup(scope, key, Some(revision))?;
        ensure!(
            start_byte < end_byte && end_byte - start_byte <= MAX_CITATION_BYTES,
            "invalid source citation range"
        );
        let stored = find_revision(&self.connection, scope, key, Some(revision))?
            .ok_or_else(|| anyhow::anyhow!("source revision not found"))?;
        // str::get rejects out-of-bounds offsets and partial multibyte scalars.
        let quote = stored
            .content
            .get(start_byte..end_byte)
            .ok_or_else(|| anyhow::anyhow!("invalid source citation range"))?;
        let start_line = 1 + stored.content.as_bytes()[..start_byte]
            .iter()
            .filter(|&&byte| byte == b'\n')
            .count();
        let end_line = 1 + stored.content.as_bytes()[..end_byte - 1]
            .iter()
            .filter(|&&byte| byte == b'\n')
            .count();
        Ok(json!({
            "schema_version": SOURCE_SCHEMA_VERSION, "source_id": stored.source_id,
            "source_key": stored.source_key, "scope": stored.scope,
            "revision_id": stored.revision_id, "revision": stored.revision,
            "content_hash": stored.content_hash, "revision_hash": stored.revision_hash,
            "start_byte": start_byte, "end_byte": end_byte,
            "start_line": start_line, "end_line": end_line, "quote": quote,
            "trust": "untrusted_source_data",
        }))
    }

    pub(crate) fn source_integrity_violations(&self) -> Result<usize> {
        let mut violations = self.connection.query_row(
            "SELECT count(*) FROM sources s WHERE NOT EXISTS(SELECT 1 FROM source_revisions r WHERE r.source_id = s.source_id) AND NOT EXISTS(SELECT 1 FROM source_lifecycle l WHERE l.source_id=s.source_id AND l.status='deleted')",
            [], |row| row.get::<_, i64>(0),
        )? as usize;
        violations += self.connection.query_row(
            "SELECT count(*) FROM (SELECT source_id FROM source_revisions GROUP BY source_id HAVING min(revision) <> 1 OR max(revision) <> count(*))",
            [], |row| row.get::<_, i64>(0),
        )? as usize;
        let mut statement = self.connection.prepare(SELECT_REVISION)?;
        let records = statement.query_map([], StoredRevision::read)?;
        for record in records {
            let record = record?;
            let actual_hash = sha256_hex(&record.content);
            if actual_hash != record.content_hash
                || record.revision_hash
                    != revision_hash(
                        &record.kind,
                        &record.format,
                        &record.eligibility,
                        record.origin.as_deref(),
                        record.title.as_deref(),
                        record.author.as_deref(),
                        &actual_hash,
                        record.published_at_ms,
                    )
            {
                violations += 1;
            }
        }
        Ok(violations)
    }
}

const SELECT_REVISION: &str = "SELECT s.source_id, s.scope, s.source_key, r.revision_id, r.revision, r.kind, r.format, r.learning_eligibility, r.origin, r.title, r.author, r.content, r.content_hash, r.revision_hash, r.published_at_ms, r.captured_at_ms, r.redactions, r.extraction_version, length(CAST(r.content AS BLOB)), l.status, l.generation FROM sources s JOIN source_revisions r ON r.source_id = s.source_id JOIN source_lifecycle l ON l.source_id=s.source_id";

fn select_summary() -> String {
    // Pagination must not load up to 100 full source bodies just to list their
    // metadata. This replacement operates only on our fixed SQL projection.
    SELECT_REVISION.replace("r.content,", "'' AS content,")
}

pub(crate) struct StoredRevision {
    pub(crate) source_id: String,
    pub(crate) scope: String,
    pub(crate) source_key: String,
    pub(crate) revision_id: String,
    pub(crate) revision: i64,
    pub(crate) kind: String,
    pub(crate) format: String,
    pub(crate) eligibility: String,
    pub(crate) origin: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) author: Option<String>,
    pub(crate) content: String,
    pub(crate) content_hash: String,
    pub(crate) revision_hash: String,
    pub(crate) published_at_ms: Option<i64>,
    pub(crate) captured_at_ms: i64,
    pub(crate) redactions: usize,
    pub(crate) extraction_version: String,
    pub(crate) byte_length: usize,
    pub(crate) lifecycle_status: String,
    pub(crate) generation: i64,
}

impl StoredRevision {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            source_id: row.get(0)?,
            scope: row.get(1)?,
            source_key: row.get(2)?,
            revision_id: row.get(3)?,
            revision: row.get(4)?,
            kind: row.get(5)?,
            format: row.get(6)?,
            eligibility: row.get(7)?,
            origin: row.get(8)?,
            title: row.get(9)?,
            author: row.get(10)?,
            content: row.get(11)?,
            content_hash: row.get(12)?,
            revision_hash: row.get(13)?,
            published_at_ms: row.get(14)?,
            captured_at_ms: row.get(15)?,
            redactions: row.get::<_, i64>(16)? as usize,
            extraction_version: row.get(17)?,
            byte_length: row.get::<_, i64>(18)? as usize,
            lifecycle_status: row.get(19)?,
            generation: row.get(20)?,
        })
    }

    pub(crate) fn summary(&self) -> Value {
        json!({
            "schema_version": SOURCE_SCHEMA_VERSION,
            "source_id": self.source_id, "scope": self.scope, "source_key": self.source_key,
            "revision_id": self.revision_id, "revision": self.revision,
            "kind": self.kind, "format": self.format, "learning_eligibility": self.eligibility,
            "origin": self.origin, "title": self.title, "author": self.author,
            "content_hash": self.content_hash, "revision_hash": self.revision_hash,
            "published_at_ms": self.published_at_ms, "captured_at_ms": self.captured_at_ms,
            "redactions": self.redactions, "extraction_version": self.extraction_version,
            "byte_length": self.byte_length,
            "trust": "untrusted_source_data",
            "lifecycle_status": self.lifecycle_status, "generation": self.generation,
        })
    }
}

pub(crate) fn find_revision(
    connection: &Connection,
    scope: &str,
    key: &str,
    revision: Option<&str>,
) -> Result<Option<StoredRevision>> {
    let sql = format!(
        "{SELECT_REVISION} WHERE s.scope = ?1 AND s.source_key = ?2 AND (?3 IS NULL OR r.revision_id = ?3) ORDER BY r.revision DESC LIMIT 1"
    );
    Ok(connection
        .query_row(&sql, params![scope, key, revision], StoredRevision::read)
        .optional()?)
}

#[allow(clippy::too_many_arguments)]
fn revision_hash(
    kind: &str,
    format: &str,
    eligibility: &str,
    origin: Option<&str>,
    title: Option<&str>,
    author: Option<&str>,
    content_hash: &str,
    published_at_ms: Option<i64>,
) -> String {
    // A typed JSON tuple supplies unambiguous separators and a versioned hash
    // domain. Raw input and timestamps chosen by the collector never enter it.
    sha256_hex(
        &json!([
            "moon-source-revision-v1",
            EXTRACTION_VERSION,
            kind,
            format,
            eligibility,
            origin,
            title,
            author,
            content_hash,
            published_at_ms
        ])
        .to_string(),
    )
}

fn enum_string(value: impl Serialize) -> String {
    serde_json::to_value(value)
        .expect("source enum serialises")
        .as_str()
        .expect("source enum is a string")
        .to_owned()
}

pub(crate) fn validate_lookup(scope: &str, key: &str, revision: Option<&str>) -> Result<()> {
    validate_identity(scope, false)?;
    validate_identity(key, true)?;
    if let Some(id) = revision {
        validate_revision_id(id)?;
    }
    Ok(())
}

pub(crate) fn validate_identity(value: &str, key: bool) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 256
            && value.trim() == value
            && !value.chars().any(char::is_control),
        "invalid source identity or scope"
    );
    if key {
        ensure!(
            value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._:/-".contains(&byte)),
            "invalid source key"
        );
    }
    ensure!(
        redact_text(value).value == value,
        "source identity or scope contains credential-shaped text"
    );
    Ok(())
}

pub(crate) fn validate_revision_id(value: &str) -> Result<()> {
    ensure!(
        value.len() == 35
            && value.starts_with("sr_")
            && value[3..]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "invalid source revision id"
    );
    Ok(())
}

fn validate_limit(limit: usize) -> Result<()> {
    ensure!(
        (1..=100).contains(&limit),
        "source page limit must be between 1 and 100"
    );
    Ok(())
}

fn validate_text(value: &str, maximum: usize, multiline: bool) -> Result<()> {
    ensure!(
        !value.trim().is_empty() && value.len() <= maximum,
        "source text is empty or exceeds its size limit"
    );
    ensure!(
        !value
            .chars()
            .any(|c| c.is_control() && !(multiline && matches!(c, '\n' | '\r' | '\t'))),
        "source text contains unsupported control characters"
    );
    Ok(())
}

fn validate_origin(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 4096
            && value.trim() == value
            && !value.chars().any(char::is_control),
        "invalid source origin"
    );
    let url = url::Url::parse(value).map_err(|_| anyhow::anyhow!("invalid source origin"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        bail!("source origin must be an HTTP(S) URL without credentials, query or fragment");
    }
    ensure!(
        redact_text(value).value == value,
        "source origin contains credential-shaped text"
    );
    Ok(())
}
