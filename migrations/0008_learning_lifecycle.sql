ALTER TABLE memory_items ADD COLUMN observed_at_ms INTEGER;
UPDATE memory_items
SET observed_at_ms = coalesce((
        SELECT min(e.completed_at_ms)
        FROM memory_citations c JOIN evidence_sessions e ON e.id = c.evidence_session_id
        WHERE c.memory_document_id = memory_items.document_id
    ), valid_from_ms),
    last_confirmed_at_ms = coalesce((
        SELECT max(e.completed_at_ms)
        FROM memory_citations c JOIN evidence_sessions e ON e.id = c.evidence_session_id
        WHERE c.memory_document_id = memory_items.document_id
    ), last_confirmed_at_ms);

CREATE TABLE memory_aliases (
    canonical_key TEXT PRIMARY KEY,
    document_id INTEGER NOT NULL REFERENCES memory_items(document_id) ON DELETE RESTRICT,
    created_at_ms INTEGER NOT NULL
);
CREATE INDEX memory_aliases_document ON memory_aliases(document_id);

CREATE TABLE learning_runs (
    run_id TEXT PRIMARY KEY,
    run_key TEXT NOT NULL,
    scope TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('prepared', 'committed', 'failed')),
    created_at_ms INTEGER NOT NULL,
    completed_at_ms INTEGER,
    lease_until_ms INTEGER,
    snapshot_hash TEXT NOT NULL,
    snapshot_json TEXT NOT NULL,
    outcome_json TEXT,
    CHECK((status = 'prepared') = (lease_until_ms IS NOT NULL))
);
CREATE INDEX learning_runs_key ON learning_runs(run_key, created_at_ms DESC);
CREATE INDEX learning_runs_lease ON learning_runs(status, lease_until_ms);

CREATE TABLE learning_processed_evidence (
    evidence_session_id INTEGER PRIMARY KEY REFERENCES evidence_sessions(id) ON DELETE RESTRICT,
    run_id TEXT NOT NULL REFERENCES learning_runs(run_id) ON DELETE RESTRICT,
    processed_at_ms INTEGER NOT NULL
);

CREATE TABLE learning_reviews (
    id INTEGER PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES learning_runs(run_id) ON DELETE RESTRICT,
    document_id INTEGER REFERENCES memory_items(document_id) ON DELETE RESTRICT,
    scope TEXT NOT NULL,
    canonical_key TEXT NOT NULL,
    reason_hash TEXT NOT NULL,
    reason TEXT NOT NULL CHECK(length(reason)<=2000),
    evidence_ids_json TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL
);

CREATE INDEX learning_reviews_document ON learning_reviews(document_id);
CREATE INDEX learning_reviews_unassigned_key ON learning_reviews(scope, canonical_key)
    WHERE document_id IS NULL;
