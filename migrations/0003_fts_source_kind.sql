DROP TABLE chunk_fts;

CREATE VIRTUAL TABLE chunk_fts USING fts5(
    title,
    content,
    source_uri UNINDEXED,
    scope UNINDEXED,
    source_kind UNINDEXED,
    tokenize = 'porter unicode61'
);

INSERT INTO chunk_fts(rowid, title, content, source_uri, scope, source_kind)
SELECT c.id, d.title, c.content, d.source_uri, d.scope, d.source_kind
FROM chunks c
JOIN documents d ON d.id = c.document_id
WHERE d.active = 1;
