ALTER TABLE embedding_queue ADD COLUMN priority INTEGER NOT NULL DEFAULT 50;
ALTER TABLE embedding_queue ADD COLUMN next_attempt_at_ms INTEGER;

UPDATE embedding_queue
SET priority = CASE
    WHEN chunk_id IN (
        SELECT c.id
        FROM chunks c
        JOIN documents d ON d.id = c.document_id
        WHERE d.source_kind = 'memory'
    ) THEN 100
    ELSE 50
END;

DELETE FROM embedding_queue
WHERE chunk_id IN (
    SELECT c.id
    FROM chunks c
    JOIN documents d ON d.id = c.document_id
    WHERE d.source_kind = 'evidence'
);

DROP INDEX IF EXISTS embedding_queue_order;
DROP INDEX IF EXISTS embedding_queue_available;
CREATE INDEX embedding_queue_available
    ON embedding_queue(next_attempt_at_ms, lease_until_ms, priority DESC, queued_at_ms, chunk_id);
