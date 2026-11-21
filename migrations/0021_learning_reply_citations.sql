-- A learning citation may carry the verified preceding assistant message only
-- alongside the human evidence it helps interpret. Historical citations remain valid.
ALTER TABLE memory_citations ADD COLUMN prior_reply TEXT
    CHECK(prior_reply IS NULL OR (json_valid(prior_reply) AND json_type(prior_reply)='object'
        AND json_type(prior_reply,'$.message_id') IS 'text' AND json_type(prior_reply,'$.quote') IS 'text'));
CREATE INDEX evidence_reply_terminal ON evidence_sessions(
    json_extract(metadata_json,'$.accepted_position.terminal')
) WHERE json_extract(metadata_json,'$.accepted_position.terminal') IS NOT NULL;

-- Reused at preparation, commit and disclosure. A second delivery or predecessor
-- invalidates the contextual link, including when it is observed after the run.
CREATE VIEW learning_reply_pairs AS
SELECT e.id AS evidence_id,p.id AS proposal_evidence_id,h.discord_message_id,h.reply FROM evidence_sessions e
JOIN documents d ON d.id=e.document_id AND d.active=1
JOIN evidence_admissions b ON b.evidence_id=e.id
JOIN privacy_admissions a ON a.admission_key=b.admission_key AND a.sender_kind='human'
JOIN evidence_sessions p ON json_extract(p.metadata_json,'$.accepted_position.terminal')
                          =json_extract(e.metadata_json,'$.accepted_position.before')
JOIN documents pd ON pd.id=p.document_id AND pd.active=1 AND pd.scope=d.scope
JOIN evidence_admissions pb ON pb.evidence_id=p.id
JOIN privacy_admissions pa ON pa.admission_key=pb.admission_key
     AND pa.sender_kind='human' AND pa.sender=a.sender AND pa.owners_json=a.owners_json
JOIN private_turn_history h ON h.admission_key=pb.admission_key
WHERE e.id<>p.id AND e.redactions=0 AND p.redactions=0
  AND length(CAST(d.body AS BLOB))<=512 AND h.accepted=1 AND h.reply IS NOT NULL
  AND length(CAST(h.reply AS BLOB))<=2048 AND h.reply_owners_json=a.owners_json
  AND h.reply_delivered_at_ms >= json_extract(p.metadata_json,'$.accepted_position.received_at_ms')
  AND json_extract(e.metadata_json,'$.accepted_position.received_at_ms')-h.reply_delivered_at_ms BETWEEN 1 AND 86400000
  AND NOT EXISTS(SELECT 1 FROM private_turn_history other WHERE other.admission_key=h.admission_key
                 AND other.reply_hash<>h.reply_hash AND other.discord_message_id IS NOT NULL)
  AND NOT EXISTS(SELECT 1 FROM evidence_sessions other
                 WHERE other.id<>p.id AND json_extract(other.metadata_json,'$.accepted_position.terminal')
                 =json_extract(p.metadata_json,'$.accepted_position.terminal'));

-- A later conflicting receipt or inactive source invalidates a saved claim
-- that depended on the reply. Apply this to every chat-facing read of its ID.
CREATE INDEX memory_citations_reply_dependencies ON memory_citations(memory_document_id)
WHERE prior_reply IS NOT NULL;
CREATE VIEW memory_ineligible_reply_dependencies AS
SELECT c.memory_document_id FROM memory_citations c
WHERE c.prior_reply IS NOT NULL AND NOT EXISTS(
    SELECT 1 FROM learning_reply_pairs p
    WHERE p.evidence_id=c.evidence_session_id
      AND p.discord_message_id=json_extract(c.prior_reply,'$.message_id')
      AND p.reply=json_extract(c.prior_reply,'$.quote')
);
DROP VIEW chat_recallable_memories;
CREATE VIEW chat_recallable_memories AS
SELECT visible.document_id,visible.scope FROM (
    SELECT document_id,scope FROM audience_recallable_memories
    UNION ALL
    SELECT o.document_id,o.scope FROM owned_legacy_memories o
    JOIN documents d ON d.id=o.document_id
    JOIN memory_items m ON m.document_id=o.document_id
    WHERE d.active=1 AND m.superseded_by IS NULL
      AND (m.valid_until_ms IS NULL OR m.valid_until_ms>CAST(unixepoch('subsec')*1000 AS INTEGER))
) visible
WHERE NOT EXISTS(SELECT 1 FROM memory_ineligible_reply_dependencies bad
                 WHERE bad.memory_document_id=visible.document_id);
