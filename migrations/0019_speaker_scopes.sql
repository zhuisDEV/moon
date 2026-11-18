-- An ACL decides who may recall. A verified speaker independently owns
-- their learning history, so personal corrections cannot mutate a co-owner's.
CREATE TABLE speaker_memory_scopes (
    scope TEXT PRIMARY KEY NOT NULL,
    audience_scope TEXT NOT NULL REFERENCES privacy_audiences(scope),
    sender TEXT NOT NULL,
    UNIQUE(audience_scope,sender)
);
CREATE TRIGGER speaker_scope_requires_owner BEFORE INSERT ON speaker_memory_scopes
WHEN NOT EXISTS(SELECT 1 FROM privacy_audiences a, json_each(a.readers_json) owner
    WHERE a.scope=NEW.audience_scope AND a.kind='private' AND owner.value=NEW.sender)
BEGIN SELECT RAISE(ABORT,'speaker must own the conversation'); END;
CREATE TRIGGER speaker_scope_immutable BEFORE UPDATE ON speaker_memory_scopes
BEGIN SELECT RAISE(ABORT,'speaker scope is immutable'); END;
CREATE TRIGGER speaker_scope_retained BEFORE DELETE ON speaker_memory_scopes
BEGIN SELECT RAISE(ABORT,'speaker scope is immutable'); END;
CREATE VIEW memory_audiences AS
SELECT scope,kind,readers_json FROM privacy_audiences UNION ALL
SELECT s.scope,a.kind,a.readers_json FROM speaker_memory_scopes s
JOIN privacy_audiences a ON a.scope=s.audience_scope;
CREATE TRIGGER privacy_new_speaker_scope AFTER INSERT ON speaker_memory_scopes
BEGIN UPDATE privacy_revision SET revision=revision+1 WHERE id=1; END;

DROP TRIGGER evidence_admissions_require_audience;
CREATE TRIGGER evidence_admissions_require_audience BEFORE INSERT ON evidence_admissions
WHEN NOT EXISTS (
    SELECT 1 FROM evidence_sessions e JOIN documents d ON d.id=e.document_id
    JOIN speaker_memory_scopes s ON s.scope=d.scope
    JOIN privacy_audiences a ON a.scope=s.audience_scope
    JOIN privacy_admissions t ON t.owners_json=a.readers_json AND t.sender=s.sender
    WHERE e.id=NEW.evidence_id AND t.admission_key=NEW.admission_key
)
BEGIN SELECT RAISE(ABORT,'evidence scope differs from admission speaker or owners'); END;

-- Keep old evidence and heads readable under their original ACL. Freeze their
-- automatic learning work, including a prepared lease from a previous binary.
CREATE TABLE historic_speaker_scopes(scope TEXT PRIMARY KEY NOT NULL);
INSERT INTO historic_speaker_scopes(scope)
SELECT DISTINCT d.scope FROM evidence_admissions b
JOIN evidence_sessions e ON e.id=b.evidence_id JOIN documents d ON d.id=e.document_id;
INSERT OR IGNORE INTO privacy_legacy_runs(run_id)
SELECT r.run_id FROM learning_runs r WHERE r.status='prepared'
AND EXISTS(SELECT 1 FROM historic_speaker_scopes h WHERE h.scope=r.scope);

DROP VIEW audience_recallable_memories;
CREATE VIEW audience_recallable_memories AS
SELECT d.id AS document_id,d.scope FROM documents d
JOIN memory_items m ON m.document_id=d.id JOIN memory_audiences a ON a.scope=d.scope
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

DROP TRIGGER canonical_document_scope_guard;
CREATE TRIGGER canonical_document_scope_guard BEFORE UPDATE OF scope ON documents
WHEN OLD.scope<>NEW.scope AND (EXISTS(SELECT 1 FROM memory_items WHERE document_id=OLD.id)
 OR EXISTS(SELECT 1 FROM evidence_sessions WHERE document_id=OLD.id)
 OR EXISTS(SELECT 1 FROM memory_audiences WHERE scope IN (OLD.scope, NEW.scope)))
BEGIN SELECT RAISE(ABORT,'managed document scope is immutable'); END;
DROP TRIGGER privacy_new_document;
CREATE TRIGGER privacy_new_document AFTER INSERT ON documents
WHEN EXISTS(SELECT 1 FROM memory_audiences a WHERE a.scope=NEW.scope)
BEGIN UPDATE privacy_revision SET revision=revision+1 WHERE id=1; END;
DROP TRIGGER privacy_changed_document;
CREATE TRIGGER privacy_changed_document AFTER UPDATE ON documents
WHEN EXISTS(SELECT 1 FROM memory_audiences a WHERE a.scope=OLD.scope)
BEGIN UPDATE privacy_revision SET revision=revision+1 WHERE id=1; END;
DROP TRIGGER privacy_changed_memory;
CREATE TRIGGER privacy_changed_memory AFTER UPDATE ON memory_items
WHEN EXISTS(SELECT 1 FROM memory_audiences a WHERE a.scope=NEW.identity_scope)
BEGIN UPDATE privacy_revision SET revision=revision+1 WHERE id=1; END;
