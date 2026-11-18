//! Role-labelled private chat history. Chat history is not a source for fact learning.
use crate::{
    Store, chunking::sha256_hex, privacy::canonical_readers, publication::validate_admission_key,
    redaction::redact_text,
};
use anyhow::{Result, ensure};
use rusqlite::{TransactionBehavior, params};
use serde_json::{Value, json};

/// Trusted Discord delivery receipt and its own independently observed audience.
pub struct ReplyDelivery<'a> {
    pub message_id: &'a str,
    pub readers: &'a [String],
    pub owners: &'a [String],
}

impl Store {
    /// The trusted adapter supplies the accepted visible final and the text
    /// confirmed by Discord independently. Either event may arrive first.
    pub fn observe_private_reply(
        &mut self,
        key: &str,
        text: &str,
        delivery: Option<ReplyDelivery<'_>>,
    ) -> Result<()> {
        validate_admission_key(key)?;
        let (message_id, readers, owners, delivered_at) = if let Some(receipt) = delivery {
            ensure!(
                (15..=20).contains(&receipt.message_id.len())
                    && receipt.message_id.bytes().all(|c| c.is_ascii_digit()),
                "invalid Discord delivery identifier"
            );
            let readers = canonical_readers(receipt.readers)?;
            let owners = canonical_readers(receipt.owners)?;
            ensure!(
                !receipt.readers.is_empty()
                    && receipt.owners.iter().all(|p| receipt.readers.contains(p)),
                "reply owners must have access to the delivery destination"
            );
            let sequence: u64 = receipt.message_id.parse()?;
            let delivered_at = 1_420_070_400_000i64 + (sequence >> 22) as i64;
            (
                Some(receipt.message_id),
                Some(readers),
                Some(owners),
                Some(delivered_at),
            )
        } else {
            (None, None, None, None)
        };
        ensure!(
            !text.trim().is_empty() && text.len() <= 65_536,
            "invalid reply size"
        );
        if redact_text(text).count > 0 {
            return Ok(());
        }
        let hash = sha256_hex(text);
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let eligible: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM privacy_admissions a WHERE a.admission_key=?1
             AND a.owners_json IS NOT NULL AND a.sender IS NOT NULL
             AND (?2 IS NOT NULL OR EXISTS(SELECT 1 FROM evidence_admissions e
                  WHERE e.admission_key=a.admission_key)))",
            params![key, message_id],
            |r| r.get(0),
        )?;
        ensure!(
            eligible,
            "private reply lacks a verified admission or accepted evidence"
        );
        // Separate immutable delivery identities prevent a retried receipt from
        // moving to another user. Repeated identical events have no side effects.
        tx.execute(
            "INSERT INTO private_turn_history(admission_key,reply_hash,accepted,discord_message_id,
                 reply_readers_json,reply_owners_json,reply_delivered_at_ms) VALUES(?1,?2,?3 IS NULL,?3,?5,?6,?7)
             ON CONFLICT(admission_key,reply_hash) DO UPDATE SET accepted=1,
               discord_message_id=coalesce(private_turn_history.discord_message_id,excluded.discord_message_id),
               reply_delivered_at_ms=coalesce(private_turn_history.reply_delivered_at_ms,excluded.reply_delivered_at_ms),
               reply_readers_json=coalesce(private_turn_history.reply_readers_json,excluded.reply_readers_json),
               reply_owners_json=coalesce(private_turn_history.reply_owners_json,excluded.reply_owners_json), reply=?4
             WHERE private_turn_history.reply IS NULL AND private_turn_history.accepted<>excluded.accepted
               AND (private_turn_history.reply_owners_json IS NOT NULL OR excluded.reply_owners_json IS NOT NULL)",
            params![key,hash,message_id,text,readers,owners,delivered_at])?;
        if let Some(id) = message_id {
            let matches: bool = tx.query_row(
                "SELECT discord_message_id IS ?3 AND reply_readers_json IS ?4 AND reply_owners_json IS ?5
                 FROM private_turn_history WHERE admission_key=?1 AND reply_hash=?2",
                params![key, hash, id, readers, owners], |r| r.get(0),
            )?;
            ensure!(
                matches,
                "conflicting or unattributable private reply delivery"
            );
        }
        tx.commit()?;
        Ok(())
    }

    /// Recent turns readable by every current destination reader. Empty or
    /// unknown audiences have no access. Bound each entry before returning it.
    pub fn private_turn_history(&self, readers: &[String], limit: usize) -> Result<Vec<Value>> {
        ensure!((1..=20).contains(&limit), "invalid history limit");
        let audience = canonical_readers(readers)?;
        if readers.is_empty() {
            return Ok(Vec::new());
        }
        // Filter both message roles independently before ranking any turns.
        // A reply owner may have no access to its preceding question, or vice versa.
        let mut query = self.connection.prepare(
            "WITH visible AS MATERIALIZED (
                SELECT e.id,e.completed_at_ms AS event_ms,0 AS role,NULL AS reply_hash
                FROM evidence_admissions x JOIN privacy_admissions a USING(admission_key)
                JOIN evidence_sessions e ON e.id=x.evidence_id JOIN documents d ON d.id=e.document_id
                WHERE d.active=1 AND a.owners_json IS NOT NULL AND NOT EXISTS
                  (SELECT value FROM json_each(?1) EXCEPT SELECT value FROM json_each(a.owners_json))
                UNION ALL
                SELECT e.id,h.reply_delivered_at_ms,1,h.reply_hash
                FROM private_turn_history h JOIN evidence_admissions x USING(admission_key)
                JOIN evidence_sessions e ON e.id=x.evidence_id JOIN documents d ON d.id=e.document_id
                WHERE d.active=1 AND h.reply IS NOT NULL AND h.reply_owners_json IS NOT NULL
                  AND h.reply_delivered_at_ms IS NOT NULL AND NOT EXISTS
                  (SELECT value FROM json_each(?1) EXCEPT SELECT value FROM json_each(h.reply_owners_json))
             ), recent AS (
                SELECT id FROM visible GROUP BY id ORDER BY max(event_ms) DESC,id DESC LIMIT ?2
             )
             SELECT v.event_ms,v.role,
                    CASE v.role WHEN 0 THEN a.sender END,
                    CASE v.role WHEN 0 THEN substr(d.body,1,2054) ELSE substr(h.reply,1,2048) END
             FROM recent r JOIN visible v USING(id) JOIN evidence_sessions e ON e.id=r.id
             JOIN evidence_admissions x ON x.evidence_id=e.id JOIN privacy_admissions a USING(admission_key)
             JOIN documents d ON d.id=e.document_id
             LEFT JOIN private_turn_history h ON h.admission_key=x.admission_key AND h.reply_hash=v.reply_hash
             ORDER BY v.event_ms,v.id,v.role"
        )?;
        Ok(query
            .query_map(params![audience, limit as i64], |r| {
                let at: i64 = r.get(0)?;
                let text: String = r.get(3)?;
                if r.get::<_, i64>(1)? == 1 {
                    return Ok(json!({"role":"assistant", "text":text, "delivered_at_ms":at}));
                }
                Ok(
                    json!({"role":"user", "text":text.strip_prefix("User:\n").unwrap_or(&text),
                "turn_sender":r.get::<_,String>(2)?, "completed_at_ms":at}),
                )
            })?
            .collect::<rusqlite::Result<_>>()?)
    }
}
