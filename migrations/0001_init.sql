CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY,
    applied_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS metadata (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS documents (
    id INTEGER PRIMARY KEY,
    source_uri TEXT NOT NULL UNIQUE,
    source_kind TEXT NOT NULL,
    scope TEXT NOT NULL DEFAULT 'global',
    title TEXT,
    content_hash TEXT NOT NULL,
    modified_at_ms INTEGER NOT NULL,
    indexed_at_ms INTEGER NOT NULL,
    active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
    metadata_json TEXT NOT NULL DEFAULT '{}'
);

CREATE INDEX IF NOT EXISTS documents_active_scope_kind
    ON documents(active, scope, source_kind);
CREATE INDEX IF NOT EXISTS documents_modified_at
    ON documents(modified_at_ms DESC);

CREATE TABLE IF NOT EXISTS chunks (
    id INTEGER PRIMARY KEY,
    document_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,
    content TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    start_byte INTEGER NOT NULL,
    end_byte INTEGER NOT NULL,
    embedding_model TEXT,
    embedded_at_ms INTEGER,
    UNIQUE(document_id, ordinal)
);

CREATE INDEX IF NOT EXISTS chunks_document ON chunks(document_id, ordinal);
CREATE INDEX IF NOT EXISTS chunks_embedding_state
    ON chunks(embedding_model, embedded_at_ms);

CREATE VIRTUAL TABLE IF NOT EXISTS chunk_fts USING fts5(
    title,
    content,
    source_uri UNINDEXED,
    scope UNINDEXED,
    tokenize = 'porter unicode61'
);

CREATE TABLE IF NOT EXISTS memory_items (
    document_id INTEGER PRIMARY KEY REFERENCES documents(id) ON DELETE CASCADE,
    memory_kind TEXT NOT NULL,
    importance REAL NOT NULL DEFAULT 0.5 CHECK (importance >= 0 AND importance <= 1),
    confidence REAL NOT NULL DEFAULT 1.0 CHECK (confidence >= 0 AND confidence <= 1),
    valid_from_ms INTEGER,
    valid_until_ms INTEGER,
    superseded_by INTEGER REFERENCES memory_items(document_id),
    pinned INTEGER NOT NULL DEFAULT 0 CHECK (pinned IN (0, 1))
);

CREATE INDEX IF NOT EXISTS memory_items_kind_active
    ON memory_items(memory_kind, superseded_by, pinned);

CREATE TABLE IF NOT EXISTS embedding_queue (
    chunk_id INTEGER PRIMARY KEY REFERENCES chunks(id) ON DELETE CASCADE,
    queued_at_ms INTEGER NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    last_error TEXT
);

CREATE INDEX IF NOT EXISTS embedding_queue_order
    ON embedding_queue(queued_at_ms, chunk_id);

CREATE TABLE IF NOT EXISTS runtime_state (
    key TEXT PRIMARY KEY,
    value_json TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS import_runs (
    id INTEGER PRIMARY KEY,
    source_root TEXT NOT NULL,
    started_at_ms INTEGER NOT NULL,
    completed_at_ms INTEGER,
    imported INTEGER NOT NULL DEFAULT 0,
    unchanged INTEGER NOT NULL DEFAULT 0,
    failed INTEGER NOT NULL DEFAULT 0,
    error TEXT
);
