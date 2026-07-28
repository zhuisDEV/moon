ALTER TABLE memory_items ADD COLUMN canonical_key TEXT;
ALTER TABLE memory_items ADD COLUMN last_confirmed_at_ms INTEGER;

UPDATE memory_items
SET last_confirmed_at_ms = valid_from_ms
WHERE last_confirmed_at_ms IS NULL;

CREATE INDEX memory_items_canonical_key
    ON memory_items(canonical_key);

CREATE TABLE evidence_sessions (
    id INTEGER PRIMARY KEY,
    session_id TEXT NOT NULL UNIQUE,
    document_id INTEGER NOT NULL UNIQUE
        REFERENCES documents(id) ON DELETE RESTRICT,
    completed_at_ms INTEGER NOT NULL,
    recorded_at_ms INTEGER NOT NULL,
    redactions INTEGER NOT NULL DEFAULT 0 CHECK (redactions >= 0),
    metadata_json TEXT NOT NULL DEFAULT '{}'
);

CREATE INDEX evidence_sessions_completed
    ON evidence_sessions(completed_at_ms DESC, id DESC);

CREATE TABLE memory_heads (
    canonical_key TEXT PRIMARY KEY,
    document_id INTEGER NOT NULL UNIQUE
        REFERENCES memory_items(document_id) ON DELETE RESTRICT,
    updated_at_ms INTEGER NOT NULL
);

CREATE TABLE memory_citations (
    id INTEGER PRIMARY KEY,
    memory_document_id INTEGER NOT NULL
        REFERENCES memory_items(document_id) ON DELETE CASCADE,
    evidence_session_id INTEGER NOT NULL
        REFERENCES evidence_sessions(id) ON DELETE RESTRICT,
    evidence_document_id INTEGER NOT NULL
        REFERENCES documents(id) ON DELETE RESTRICT,
    start_byte INTEGER NOT NULL CHECK (start_byte >= 0),
    end_byte INTEGER NOT NULL CHECK (end_byte > start_byte),
    start_line INTEGER NOT NULL CHECK (start_line >= 1),
    end_line INTEGER NOT NULL CHECK (end_line >= start_line),
    quote TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    UNIQUE(memory_document_id, evidence_session_id, start_byte, end_byte)
);

CREATE INDEX memory_citations_memory
    ON memory_citations(memory_document_id, created_at_ms DESC, id DESC);
