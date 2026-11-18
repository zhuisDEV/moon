-- Test fixtures only: reconstruct schema 14 before exercising older upgrade paths.
DROP VIEW chat_recallable_memories;
DROP VIEW owned_legacy_memories;
DROP TRIGGER privacy_recovery_owner_scope;
DROP TABLE legacy_collection_owner;
DROP TABLE private_turn_history;
DELETE FROM schema_migrations WHERE version IN (17,18);
DROP TABLE admission_room_activity;
DROP TRIGGER privacy_admissions_require_owners;
DROP TABLE evidence_admissions;
DELETE FROM schema_migrations WHERE version=16;
DROP TRIGGER privacy_new_audience;
DROP TRIGGER privacy_new_admission;
DROP TRIGGER privacy_new_document;
DROP TRIGGER privacy_changed_document;
DROP TRIGGER privacy_changed_memory;
DROP TABLE privacy_revision;
DROP VIEW audience_recallable_memories;
DROP TABLE memory_publications;
DROP TABLE privacy_recoveries;
DROP TABLE privacy_legacy_runs;
DROP TABLE privacy_legacy_documents;
DROP TABLE privacy_admissions;
DROP TABLE privacy_audiences;
DROP TRIGGER memory_alias_scope_insert;
DROP TRIGGER memory_alias_scope_update;
DROP TRIGGER memory_supersession_scope;
DROP TRIGGER memory_heads_require_active_insert;
DROP TRIGGER memory_heads_require_active_update;
DROP TRIGGER memory_items_scope_guard;
DROP TRIGGER memory_items_set_identity_scope;
DROP TRIGGER canonical_document_scope_guard;
DROP INDEX memory_items_one_active_canonical_key;
ALTER TABLE memory_items DROP COLUMN identity_scope;
CREATE UNIQUE INDEX memory_items_one_active_canonical_key
    ON memory_items(canonical_key)
    WHERE canonical_key IS NOT NULL AND superseded_by IS NULL;

ALTER TABLE memory_heads RENAME TO fixture_scoped_heads;
CREATE TABLE memory_heads (
    canonical_key TEXT PRIMARY KEY,
    document_id INTEGER NOT NULL UNIQUE
        REFERENCES memory_items(document_id) ON DELETE RESTRICT,
    updated_at_ms INTEGER NOT NULL
);

INSERT INTO memory_heads SELECT canonical_key,document_id,updated_at_ms FROM fixture_scoped_heads;
DROP TABLE fixture_scoped_heads;
CREATE TRIGGER memory_heads_require_active_insert
BEFORE INSERT ON memory_heads
BEGIN
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1
        FROM memory_items m
        WHERE m.document_id = NEW.document_id
          AND m.canonical_key = NEW.canonical_key
          AND m.superseded_by IS NULL
    )
    THEN RAISE(ABORT, 'memory head must reference the active canonical memory')
    END;
END;

CREATE TRIGGER memory_heads_require_active_update
BEFORE UPDATE OF canonical_key, document_id ON memory_heads
BEGIN
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1
        FROM memory_items m
        WHERE m.document_id = NEW.document_id
          AND m.canonical_key = NEW.canonical_key
          AND m.superseded_by IS NULL
    )
    THEN RAISE(ABORT, 'memory head must reference the active canonical memory')
    END;
END;

DROP INDEX memory_aliases_document;
ALTER TABLE memory_aliases RENAME TO fixture_scoped_aliases;
CREATE TABLE memory_aliases (
    canonical_key TEXT PRIMARY KEY,
    document_id INTEGER NOT NULL REFERENCES memory_items(document_id) ON DELETE RESTRICT,
    created_at_ms INTEGER NOT NULL
);
CREATE INDEX memory_aliases_document ON memory_aliases(document_id);

INSERT INTO memory_aliases SELECT canonical_key,document_id,created_at_ms FROM fixture_scoped_aliases;
DROP TABLE fixture_scoped_aliases;
DELETE FROM schema_migrations WHERE version=15;
PRAGMA user_version=14;
