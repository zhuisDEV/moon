CREATE UNIQUE INDEX memory_items_one_active_canonical_key
    ON memory_items(canonical_key)
    WHERE canonical_key IS NOT NULL AND superseded_by IS NULL;

CREATE TRIGGER memory_items_reject_supersession_cycles
BEFORE UPDATE OF superseded_by ON memory_items
WHEN NEW.superseded_by IS NOT NULL
BEGIN
    SELECT CASE WHEN EXISTS (
        WITH RECURSIVE chain(document_id) AS (
            SELECT NEW.superseded_by
            UNION
            SELECT m.superseded_by
            FROM memory_items m
            JOIN chain c ON m.document_id = c.document_id
            WHERE m.superseded_by IS NOT NULL
        )
        SELECT 1 FROM chain WHERE document_id = NEW.document_id
    )
    THEN RAISE(ABORT, 'memory supersession cycle')
    END;
END;

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

ALTER TABLE embedding_queue ADD COLUMN claimed_by TEXT;
ALTER TABLE embedding_queue ADD COLUMN lease_until_ms INTEGER;
ALTER TABLE embedding_queue ADD COLUMN last_attempt_at_ms INTEGER;

CREATE INDEX embedding_queue_available
    ON embedding_queue(lease_until_ms, queued_at_ms, chunk_id);
