-- Test-only reconstruction of the schema-18 ACL views and guards.
DROP TABLE master_discord_principals;
DROP TRIGGER bot_evidence_requires_role;
DROP TRIGGER speaker_scope_requires_admission;
DROP TRIGGER private_audiences_require_human_owners;
DROP TRIGGER privacy_admissions_require_owners;
ALTER TABLE privacy_admissions DROP COLUMN sender_kind;
CREATE TRIGGER privacy_admissions_require_owners BEFORE INSERT ON privacy_admissions
WHEN NEW.owners_json IS NULL
    OR NOT EXISTS(SELECT 1 FROM json_each(NEW.owners_json) WHERE value=NEW.sender)
    OR EXISTS(SELECT value FROM json_each(NEW.owners_json)
              EXCEPT SELECT value FROM json_each(NEW.audience_json))
BEGIN SELECT RAISE(ABORT,'admission owners must include sender and be verified readers'); END;
DELETE FROM schema_migrations WHERE version=20;
DROP TRIGGER evidence_admissions_require_audience;
DROP VIEW audience_recallable_memories;
DROP TRIGGER canonical_document_scope_guard;
DROP TRIGGER privacy_new_document;
DROP TRIGGER privacy_changed_document;
DROP TRIGGER privacy_changed_memory;
DROP VIEW memory_audiences;
DROP TABLE speaker_memory_scopes;
DROP TABLE historic_speaker_scopes;
DELETE FROM schema_migrations WHERE version=19;
PRAGMA user_version=18;
CREATE VIEW audience_recallable_memories AS
SELECT d.id AS document_id,d.scope FROM documents d
JOIN memory_items m ON m.document_id=d.id JOIN privacy_audiences a ON a.scope=d.scope
WHERE d.active=1 AND m.superseded_by IS NULL AND
 (m.valid_until_ms IS NULL OR m.valid_until_ms>CAST(unixepoch('subsec')*1000 AS INTEGER))
 AND NOT EXISTS(SELECT 1 FROM privacy_legacy_documents l WHERE l.document_id=d.id)
 AND (
    (a.kind='public' AND EXISTS(SELECT 1 FROM memory_publications p WHERE p.document_id=d.id))
 OR (a.kind='private'
     AND EXISTS(SELECT 1 FROM memory_citations c WHERE c.memory_document_id=d.id)
     AND NOT EXISTS(SELECT 1 FROM memory_citations c
         LEFT JOIN evidence_sessions e ON e.id=c.evidence_session_id
         LEFT JOIN documents ed ON ed.id=c.evidence_document_id
         WHERE c.memory_document_id=d.id AND (e.document_id IS NOT c.evidence_document_id
        OR ed.scope IS NOT d.scope OR ed.source_kind IS NOT 'evidence'
        OR EXISTS(SELECT 1 FROM privacy_legacy_documents l WHERE l.document_id=ed.id))))
 );

CREATE TRIGGER evidence_admissions_require_audience BEFORE INSERT ON evidence_admissions
WHEN NOT EXISTS (
    SELECT 1 FROM evidence_sessions e JOIN documents d ON d.id=e.document_id
    JOIN privacy_audiences a ON a.scope=d.scope AND a.kind='private'
    JOIN privacy_admissions t ON t.owners_json=a.readers_json
    WHERE e.id=NEW.evidence_id AND t.admission_key=NEW.admission_key
)
BEGIN SELECT RAISE(ABORT,'evidence scope differs from admission owners'); END;
CREATE TRIGGER canonical_document_scope_guard BEFORE UPDATE OF scope ON documents
WHEN OLD.scope<>NEW.scope AND (EXISTS(SELECT 1 FROM memory_items WHERE document_id=OLD.id)
 OR EXISTS(SELECT 1 FROM evidence_sessions WHERE document_id=OLD.id)
 OR EXISTS(SELECT 1 FROM privacy_audiences WHERE scope IN (OLD.scope, NEW.scope)))
BEGIN SELECT RAISE(ABORT,'managed document scope is immutable'); END;
CREATE TRIGGER privacy_new_document AFTER INSERT ON documents
WHEN EXISTS(SELECT 1 FROM privacy_audiences a WHERE a.scope=NEW.scope)
BEGIN UPDATE privacy_revision SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER privacy_changed_document AFTER UPDATE ON documents
WHEN EXISTS(SELECT 1 FROM privacy_audiences a WHERE a.scope=OLD.scope)
BEGIN UPDATE privacy_revision SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER privacy_changed_memory AFTER UPDATE ON memory_items
WHEN EXISTS(SELECT 1 FROM privacy_audiences a WHERE a.scope=NEW.identity_scope)
BEGIN UPDATE privacy_revision SET revision=revision+1 WHERE id=1; END;
