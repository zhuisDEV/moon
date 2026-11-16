-- Captured sources are deliberately separate from mutable search documents and
-- completed-session evidence. Intake alone grants neither recall nor learning.
CREATE TABLE sources (
    source_id TEXT PRIMARY KEY,
    scope TEXT NOT NULL CHECK(length(CAST(scope AS BLOB)) BETWEEN 1 AND 256),
    source_key TEXT NOT NULL CHECK(length(CAST(source_key AS BLOB)) BETWEEN 1 AND 256),
    created_at_ms INTEGER NOT NULL CHECK(created_at_ms >= 0),
    UNIQUE(scope, source_key)
);

CREATE TABLE source_revisions (
    revision_id TEXT PRIMARY KEY,
    source_id TEXT NOT NULL REFERENCES sources(source_id) ON DELETE RESTRICT,
    revision INTEGER NOT NULL CHECK(revision > 0),
    kind TEXT NOT NULL CHECK(kind IN ('external_document', 'user_statement', 'reference', 'generated_analysis')),
    format TEXT NOT NULL CHECK(format IN ('text', 'markdown')),
    learning_eligibility TEXT NOT NULL CHECK(learning_eligibility IN ('reference_only', 'eligible')),
    origin TEXT,
    title TEXT,
    author TEXT,
    content TEXT NOT NULL CHECK(length(CAST(content AS BLOB)) BETWEEN 1 AND 1048576),
    content_hash TEXT NOT NULL CHECK(length(content_hash) = 64 AND content_hash NOT GLOB '*[^0-9a-f]*'),
    revision_hash TEXT NOT NULL CHECK(length(revision_hash) = 64 AND revision_hash NOT GLOB '*[^0-9a-f]*'),
    published_at_ms INTEGER CHECK(published_at_ms >= 0),
    captured_at_ms INTEGER NOT NULL CHECK(captured_at_ms >= 0),
    redactions INTEGER NOT NULL CHECK(redactions >= 0),
    extraction_version TEXT NOT NULL CHECK(extraction_version = 'moon-text-v1'),
    UNIQUE(source_id, revision)
);

-- The highest revision is the current head. Appending inside one immediate
-- transaction changes that head atomically, without a nullable/cyclic pointer.
CREATE TRIGGER source_revisions_require_sequence
BEFORE INSERT ON source_revisions
BEGIN
    SELECT CASE WHEN NEW.revision <> (
        SELECT coalesce(max(revision), 0) + 1 FROM source_revisions WHERE source_id = NEW.source_id
    ) THEN RAISE(ABORT, 'source revision sequence conflict') END;
END;

CREATE TRIGGER sources_immutable_update BEFORE UPDATE ON sources
BEGIN SELECT RAISE(ABORT, 'source identity is immutable'); END;
CREATE TRIGGER sources_immutable_delete BEFORE DELETE ON sources
BEGIN SELECT RAISE(ABORT, 'source identity is immutable'); END;
CREATE TRIGGER source_revisions_immutable_update BEFORE UPDATE ON source_revisions
BEGIN SELECT RAISE(ABORT, 'source revision is immutable'); END;
CREATE TRIGGER source_revisions_immutable_delete BEFORE DELETE ON source_revisions
BEGIN SELECT RAISE(ABORT, 'source revision is immutable'); END;
