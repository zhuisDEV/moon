-- An audience is minted only after the host verifies its readers. Exact scope
-- names are allocated in the same transaction, away from legacy namespaces.
CREATE TABLE privacy_audiences (
    scope TEXT PRIMARY KEY NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('private','public')),
    readers_json TEXT NOT NULL UNIQUE CHECK(json_valid(readers_json) AND json_type(readers_json)='array'),
    CHECK ((kind='public') = (json_array_length(readers_json)=0))
);
CREATE TRIGGER privacy_audiences_immutable BEFORE UPDATE ON privacy_audiences
BEGIN SELECT RAISE(ABORT,'memory audiences are immutable'); END;
CREATE TRIGGER privacy_audiences_retained BEFORE DELETE ON privacy_audiences
BEGIN SELECT RAISE(ABORT,'memory audiences are immutable'); END;
CREATE TABLE privacy_admissions (
    admission_key TEXT PRIMARY KEY NOT NULL,
    audience_json TEXT NOT NULL CHECK(json_valid(audience_json))
);

-- Retain the original documents, evidence and index structures for local
-- operator review. They are never imported into the fresh public audience.
CREATE TABLE privacy_legacy_documents (
    document_id INTEGER PRIMARY KEY REFERENCES documents(id) ON DELETE RESTRICT
);
INSERT INTO privacy_legacy_documents SELECT id FROM documents WHERE scope='global';
CREATE TABLE privacy_legacy_runs (
    run_id TEXT PRIMARY KEY REFERENCES learning_runs(run_id) ON DELETE RESTRICT
);
INSERT INTO privacy_legacy_runs SELECT run_id FROM learning_runs WHERE scope='global' AND status='prepared';
CREATE TABLE privacy_recoveries (
    legacy_document_id INTEGER PRIMARY KEY REFERENCES privacy_legacy_documents(document_id),
    document_id INTEGER NOT NULL REFERENCES memory_items(document_id),
    scope TEXT NOT NULL REFERENCES privacy_audiences(scope),
    quote_hash TEXT NOT NULL
);
CREATE TABLE memory_publications (
    document_id INTEGER PRIMARY KEY REFERENCES memory_items(document_id),
    actor TEXT NOT NULL, request_key TEXT NOT NULL, UNIQUE(actor,request_key)
);

-- The same personal preference key may have independent heads in two DMs.
ALTER TABLE memory_items ADD COLUMN identity_scope TEXT;
UPDATE memory_items AS m SET identity_scope=(SELECT scope FROM documents WHERE id=m.document_id);
DROP INDEX memory_items_one_active_canonical_key;
CREATE UNIQUE INDEX memory_items_one_active_canonical_key ON memory_items(identity_scope,canonical_key)
    WHERE canonical_key IS NOT NULL AND superseded_by IS NULL;
CREATE TRIGGER memory_items_set_identity_scope AFTER INSERT ON memory_items
BEGIN
    UPDATE memory_items SET identity_scope=(SELECT scope FROM documents WHERE id=NEW.document_id)
    WHERE document_id=NEW.document_id;
END;
CREATE TRIGGER memory_items_scope_guard BEFORE UPDATE OF identity_scope ON memory_items
WHEN NEW.identity_scope IS NOT (SELECT scope FROM documents WHERE id=NEW.document_id)
BEGIN SELECT RAISE(ABORT,'memory scope mismatch'); END;
CREATE TRIGGER canonical_document_scope_guard BEFORE UPDATE OF scope ON documents
WHEN OLD.scope<>NEW.scope AND (EXISTS(SELECT 1 FROM memory_items WHERE document_id=OLD.id)
 OR EXISTS(SELECT 1 FROM evidence_sessions WHERE document_id=OLD.id)
 OR EXISTS(SELECT 1 FROM privacy_audiences WHERE scope IN (OLD.scope, NEW.scope)))
BEGIN SELECT RAISE(ABORT,'managed document scope is immutable'); END;

DROP TRIGGER memory_heads_require_active_insert;
DROP TRIGGER memory_heads_require_active_update;
ALTER TABLE memory_heads RENAME TO old_memory_heads;
CREATE TABLE memory_heads (
    scope TEXT NOT NULL, canonical_key TEXT NOT NULL,
    document_id INTEGER NOT NULL UNIQUE REFERENCES memory_items(document_id) ON DELETE RESTRICT,
    updated_at_ms INTEGER NOT NULL, PRIMARY KEY(scope,canonical_key)
);
INSERT INTO memory_heads SELECT d.scope,h.canonical_key,h.document_id,h.updated_at_ms
FROM old_memory_heads h JOIN documents d ON d.id=h.document_id;
DROP TABLE old_memory_heads;
CREATE TRIGGER memory_heads_require_active_insert BEFORE INSERT ON memory_heads
WHEN NOT EXISTS(SELECT 1 FROM memory_items m WHERE m.document_id=NEW.document_id AND
    m.canonical_key=NEW.canonical_key AND m.identity_scope=NEW.scope AND m.superseded_by IS NULL)
BEGIN SELECT RAISE(ABORT,'memory head must reference an active scoped memory'); END;
CREATE TRIGGER memory_heads_require_active_update BEFORE UPDATE ON memory_heads
WHEN NOT EXISTS(SELECT 1 FROM memory_items m WHERE m.document_id=NEW.document_id AND
    m.canonical_key=NEW.canonical_key AND m.identity_scope=NEW.scope AND m.superseded_by IS NULL)
BEGIN SELECT RAISE(ABORT,'memory head must reference an active scoped memory'); END;

ALTER TABLE memory_aliases RENAME TO old_memory_aliases;
CREATE TABLE memory_aliases (
    scope TEXT NOT NULL, canonical_key TEXT NOT NULL,
    document_id INTEGER NOT NULL REFERENCES memory_items(document_id) ON DELETE RESTRICT,
    created_at_ms INTEGER NOT NULL, PRIMARY KEY(scope,canonical_key)
);
INSERT INTO memory_aliases SELECT d.scope,a.canonical_key,a.document_id,a.created_at_ms
FROM old_memory_aliases a JOIN documents d ON d.id=a.document_id;
DROP TABLE old_memory_aliases;
CREATE INDEX memory_aliases_document ON memory_aliases(document_id);
CREATE TRIGGER memory_alias_scope_insert BEFORE INSERT ON memory_aliases
WHEN NEW.scope IS NOT (SELECT identity_scope FROM memory_items WHERE document_id=NEW.document_id)
BEGIN SELECT RAISE(ABORT,'memory alias scope mismatch'); END;
CREATE TRIGGER memory_alias_scope_update BEFORE UPDATE ON memory_aliases
WHEN NEW.scope IS NOT (SELECT identity_scope FROM memory_items WHERE document_id=NEW.document_id)
BEGIN SELECT RAISE(ABORT,'memory alias scope mismatch'); END;
CREATE TRIGGER memory_supersession_scope BEFORE UPDATE OF superseded_by ON memory_items
WHEN NEW.superseded_by IS NOT NULL AND NEW.identity_scope IS NOT
    (SELECT identity_scope FROM memory_items WHERE document_id=NEW.superseded_by)
BEGIN SELECT RAISE(ABORT,'memory supersession scope mismatch'); END;

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

-- Rollback after a candidate restart must preserve newly accepted privacy
-- decisions, including a withdrawal which only flips a document's active bit.
-- The updater can compare one counter without copying conversation identifiers.
CREATE TABLE privacy_revision (
    id INTEGER PRIMARY KEY CHECK(id=1),
    revision INTEGER NOT NULL CHECK(revision>=0)
);
INSERT INTO privacy_revision VALUES(1,0);
CREATE TRIGGER privacy_new_audience AFTER INSERT ON privacy_audiences
BEGIN UPDATE privacy_revision SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER privacy_new_admission AFTER INSERT ON privacy_admissions
BEGIN UPDATE privacy_revision SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER privacy_new_document AFTER INSERT ON documents
WHEN EXISTS(SELECT 1 FROM privacy_audiences a WHERE a.scope=NEW.scope)
BEGIN UPDATE privacy_revision SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER privacy_changed_document AFTER UPDATE ON documents
WHEN EXISTS(SELECT 1 FROM privacy_audiences a WHERE a.scope=OLD.scope)
BEGIN UPDATE privacy_revision SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER privacy_changed_memory AFTER UPDATE ON memory_items
WHEN EXISTS(SELECT 1 FROM privacy_audiences a WHERE a.scope=NEW.identity_scope)
BEGIN UPDATE privacy_revision SET revision=revision+1 WHERE id=1; END;
