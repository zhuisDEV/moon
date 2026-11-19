//! Trusted host operations. Reader and actor identities must come from the
//! transport; these APIs do not authenticate a caller running the local CLI.
use crate::Store;
use crate::chunking::sha256_hex;
use crate::config::MasterConfig;
use crate::model::IngestDocument;
use crate::privacy::{canonical_readers, validate_reader};
use crate::redaction::redact_text;
use crate::store::{ingest_prepared, now_ms, prepare_ingest, random_nonce};
use anyhow::{Result, ensure};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};

pub(crate) fn validate_admission_key(key: &str) -> Result<()> {
    ensure!(
        key.len() == 64
            && key
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "invalid admission key"
    );
    Ok(())
}

/// Immutable observations from the host at admission time.
#[derive(Default, serde::Serialize)]
pub struct TurnAdmission {
    pub audience: Option<Vec<String>>,
    pub sender: Option<String>,
    pub owners: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender_kind: Option<String>,
}

impl Store {
    /// Preserve the first verified access snapshot for this question.
    /// A retry cannot change its sender, recipients, or private owners.
    pub fn bind_turn_owners(
        &mut self,
        key: &str,
        readers: &[String],
        sender: &str,
        owners: &[String],
    ) -> Result<Vec<String>> {
        self.bind_admission(key, readers, sender, owners, "human", None)
    }

    /// A trusted profile binding can adopt an existing personal Discord scope.
    pub fn bind_master_turn_owners(
        &mut self,
        key: &str,
        readers: &[String],
        sender: &str,
        owners: &[String],
        master: &MasterConfig,
    ) -> Result<Vec<String>> {
        self.bind_admission(key, readers, sender, owners, "human", Some(master))
    }

    /// The transport verified the external bot author and the owner's access
    /// to this message. A bot never acquires ownership of the resulting memory.
    pub fn bind_bot_turn(
        &mut self,
        key: &str,
        readers: &[String],
        sender: &str,
        master: &MasterConfig,
    ) -> Result<Vec<String>> {
        ensure!(
            sender
                .strip_prefix("bot:discord:")
                .is_some_and(crate::config::is_discord_id),
            "invalid external Discord bot principal"
        );
        self.bind_admission(
            key,
            readers,
            sender,
            std::slice::from_ref(&master.memory_principal),
            "bot",
            Some(master),
        )
    }

    fn bind_admission(
        &mut self,
        key: &str,
        readers: &[String],
        sender: &str,
        owners: &[String],
        kind: &str,
        master: Option<&MasterConfig>,
    ) -> Result<Vec<String>> {
        validate_admission_key(key)?;
        let audience = canonical_readers(readers)?;
        let owner_set = canonical_readers(owners)?;
        ensure!(
            kind != "human" || (!sender.starts_with("bot:") && owners.iter().any(|p| p == sender)),
            "human messages must include the verified speaker among their owners"
        );
        if let Some(master) = master {
            ensure!(
                owners.contains(&master.memory_principal),
                "Master-bound admissions must include the configured memory principal among their owners"
            );
        }
        ensure!(
            readers.iter().any(|p| p == sender)
                && owners
                    .iter()
                    .all(|p| !p.starts_with("bot:") && readers.contains(p)),
            "source and human owners must be verified readers"
        );
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO privacy_admissions(admission_key,audience_json,sender,owners_json,room_access_at_ms,sender_kind)
             VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(admission_key) DO NOTHING",
            params![key,audience,sender,owner_set,now_ms(),kind],
        )?;
        let matches: bool = tx.query_row(
            "SELECT room_access_at_ms IS NOT NULL AND audience_json=?2 AND sender IS ?3 AND owners_json IS ?4 AND sender_kind=?5
             FROM privacy_admissions WHERE admission_key=?1",
            params![key,audience,sender,owner_set,kind], |row| row.get(0),
        )?;
        ensure!(
            matches,
            "conflicting sender, owners or audience for this question"
        );
        if let Some(master) = master
            && master.memory_principal.starts_with("discord:")
        {
            tx.execute(
                "INSERT INTO master_discord_principals(principal) VALUES(?1)
                 ON CONFLICT(principal) DO NOTHING",
                [&master.memory_principal],
            )?;
        }
        tx.commit()?;
        Ok(serde_json::from_str(&owner_set)?)
    }

    pub fn turn_admission(&self, key: &str) -> Result<TurnAdmission> {
        validate_admission_key(key)?;
        let row = self.connection.query_row(
            "SELECT audience_json,sender,owners_json,sender_kind FROM privacy_admissions WHERE admission_key=?1",
            [key], |r| Ok((r.get::<_,String>(0)?, r.get(1)?, r.get::<_,Option<String>>(2)?, r.get::<_,String>(3)?))
        ).optional()?;
        let Some((audience, sender, owners, kind)) = row else {
            return Ok(TurnAdmission::default());
        };
        Ok(TurnAdmission {
            audience: Some(serde_json::from_str(&audience)?),
            sender,
            owners: owners.as_deref().map(serde_json::from_str).transpose()?,
            sender_kind: (kind == "bot").then_some(kind),
        })
    }

    /// Read-only operator index. Only an immutable evidence/admission association supplies authorship.
    pub fn evidence_by_sender(&self, sender: &str, after: i64, limit: usize) -> Result<Vec<Value>> {
        validate_reader(sender)?;
        ensure!(
            after >= 0 && (1..=100).contains(&limit),
            "invalid evidence page"
        );
        let since: i64 = if after == 0 {
            0
        } else {
            self.connection.query_row(
                "SELECT e.completed_at_ms FROM evidence_sessions e JOIN evidence_admissions x ON x.evidence_id=e.id
                 JOIN privacy_admissions t USING(admission_key) WHERE e.id=?1 AND t.sender=?2",
                params![after,sender], |r| r.get(0)
            ).optional()?.ok_or_else(|| anyhow::anyhow!("unknown cursor for this sender"))?
        };
        let mut stmt = self.connection.prepare(
            "SELECT e.id,e.completed_at_ms,json_array_length(t.audience_json),
                    (SELECT count(DISTINCT c.memory_document_id) FROM memory_citations c WHERE c.evidence_session_id=e.id),
                    t.owners_json,t.room_access_at_ms,
                    (SELECT json_group_array(json_object(
                         'message_id', h.discord_message_id,
                         'delivered_at_ms', h.reply_delivered_at_ms,
                         'owners', json(h.reply_owners_json)) ORDER BY h.reply_delivered_at_ms,h.discord_message_id)
                     FROM private_turn_history h WHERE h.admission_key=t.admission_key
                       AND h.reply IS NOT NULL AND h.reply_owners_json IS NOT NULL)
             FROM privacy_admissions t JOIN evidence_admissions x USING(admission_key)
             JOIN evidence_sessions e ON e.id=x.evidence_id
             WHERE t.sender=?1 AND (e.completed_at_ms,e.id)>(?2,?3)
             ORDER BY e.completed_at_ms,e.id LIMIT ?4"
        )?;
        let mut result = Vec::new();
        let mut rows = stmt.query(params![sender, since, after, limit as i64])?;
        while let Some(row) = rows.next()? {
            let encoded: Option<String> = row.get(4)?;
            let owners = encoded
                .as_deref()
                .map(serde_json::from_str::<Vec<String>>)
                .transpose()?;
            result.push(json!({
                "evidence_id": row.get::<_, i64>(0)?, "completed_at_ms": row.get::<_, i64>(1)?,
                "reader_count": row.get::<_, i64>(2)?, "citing_memories": row.get::<_, i64>(3)?,
                "owners": owners, "question_access_at_ms": row.get::<_,Option<i64>>(5)?,
                "reply_deliveries": serde_json::from_str::<Vec<Value>>(&row.get::<_,String>(6)?)?
            }));
        }
        Ok(result)
    }

    pub fn publish_memory(
        &mut self,
        actor: &str,
        kind: &str,
        text: &str,
        request_key: Option<&str>,
    ) -> Result<Value> {
        validate_reader(actor)?;
        ensure!(
            matches!(kind, "fact" | "skill"),
            "public memory kind must be fact or skill"
        );
        ensure!(
            !text.trim().is_empty() && text.len() <= 8192,
            "public wording must be 1–8192 bytes"
        );
        ensure!(
            redact_text(text).count == 0,
            "public wording contains a recognised credential"
        );
        let request_key = match request_key {
            Some(key) => {
                ensure!(
                    !key.trim().is_empty() && key.len() <= 512,
                    "invalid publication request key"
                );
                format!("event:{}", sha256_hex(key))
            }
            None => format!(
                "wording:{}",
                sha256_hex(&serde_json::to_string(&(kind, text))?)
            ),
        };
        let public = self.public_memory_scope()?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = tx
            .query_row(
                "SELECT d.id,d.active,d.body,m.memory_kind FROM memory_publications p
             JOIN documents d ON d.id=p.document_id JOIN memory_items m ON m.document_id=d.id
             WHERE p.actor=?1 AND p.request_key=?2",
                params![actor, request_key],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, bool>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?;
        if let Some((id, active, body, prior_kind)) = existing {
            ensure!(
                text == body && kind == prior_kind,
                "publication request already has different wording or kind"
            );
            return Ok(json!({"document_id": id, "active":active}));
        }
        let now = now_ms();
        let nonce = random_nonce(&tx)?;
        let prepared = prepare_ingest(IngestDocument {
            source_uri: format!("memory://publication/{nonce}"),
            source_kind: "memory".into(),
            scope: public,
            title: None,
            content: text.into(),
            modified_at_ms: now,
            metadata_json: "{}".into(),
        })?;
        let id = ingest_prepared(&tx, prepared)?.document_id;
        tx.execute(
            "INSERT INTO memory_items(document_id, memory_kind, importance, confidence, valid_from_ms, pinned)
             VALUES (?1, ?2, 0.5, 1.0, ?3, 0)",
            params![id, kind, now],
        )?;
        tx.execute(
            "INSERT INTO memory_publications(document_id,actor,request_key) VALUES(?1,?2,?3)",
            params![id, actor, request_key],
        )?;
        tx.commit()?;
        Ok(json!({"document_id":id,"active":true}))
    }

    pub fn withdraw_publication(&mut self, actor: &str, id: i64) -> Result<Value> {
        validate_reader(actor)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let owned: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM memory_publications WHERE actor=?1 AND document_id=?2)",
            params![actor, id],
            |r| r.get(0),
        )?;
        ensure!(owned, "no publication owned by this actor at that ID");
        tx.execute("UPDATE documents SET active=0 WHERE id=?1", [id])?;
        for sql in [
            "DELETE FROM chunk_fts WHERE rowid IN (SELECT id FROM chunks WHERE document_id=?1)",
            "DELETE FROM chunk_vectors WHERE rowid IN (SELECT id FROM chunks WHERE document_id=?1)",
            "DELETE FROM embedding_queue WHERE chunk_id IN (SELECT id FROM chunks WHERE document_id=?1)",
        ] {
            tx.execute(sql, [id])?;
        }
        tx.commit()?;
        Ok(json!({"document_id":id,"active":false}))
    }

    pub fn public_memories(&self, after: i64, limit: usize) -> Result<Vec<Value>> {
        ensure!(
            after >= 0 && (1..=100).contains(&limit),
            "invalid publication page"
        );
        let mut stmt = self.connection.prepare(
            "SELECT d.id,m.memory_kind,d.body FROM memory_publications p
            JOIN documents d ON d.id=p.document_id JOIN memory_items m ON m.document_id=d.id
            WHERE d.active=1 AND d.id>?1 ORDER BY d.id LIMIT ?2",
        )?;
        let page = stmt.query_map(params![after, limit as i64], |row| {
            let document_id: i64 = row.get(0)?;
            let kind: String = row.get(1)?;
            let text: String = row.get(2)?;
            Ok(json!({ "document_id": document_id, "kind": kind, "text": text }))
        })?;
        Ok(page.collect::<rusqlite::Result<_>>()?)
    }
}
