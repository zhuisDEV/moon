-- Source capture remains separate from session evidence. Lifecycle state is
-- mutable; retained source revisions and accepted statement text are not.
CREATE TABLE source_lifecycle (
    source_id TEXT PRIMARY KEY REFERENCES sources(source_id) ON DELETE RESTRICT,
    status TEXT NOT NULL CHECK(status IN ('active', 'withdrawn', 'deleted')),
    generation INTEGER NOT NULL CHECK(generation > 0),
    reason TEXT NOT NULL DEFAULT '' CHECK(length(CAST(reason AS BLOB)) <= 4096),
    updated_at_ms INTEGER NOT NULL
);
INSERT INTO source_lifecycle(source_id,status,generation,updated_at_ms)
SELECT source_id,'active',1,created_at_ms FROM sources;

CREATE TABLE source_lifecycle_events (
    source_id TEXT NOT NULL REFERENCES sources(source_id) ON DELETE RESTRICT,
    generation INTEGER NOT NULL CHECK(generation > 0),
    action TEXT NOT NULL CHECK(action IN ('capture','withdraw','restore','delete')),
    reason TEXT NOT NULL CHECK(length(CAST(reason AS BLOB)) <= 4096),
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY(source_id,generation)
);
INSERT INTO source_lifecycle_events SELECT source_id,generation,'capture','',updated_at_ms FROM source_lifecycle;
CREATE TRIGGER source_lifecycle_events_immutable_update BEFORE UPDATE ON source_lifecycle_events
BEGIN SELECT RAISE(ABORT,'source lifecycle history is immutable'); END;
CREATE TRIGGER source_lifecycle_events_immutable_delete BEFORE DELETE ON source_lifecycle_events
WHEN NOT EXISTS(SELECT 1 FROM source_lifecycle l WHERE l.source_id=OLD.source_id AND l.status='deleted')
BEGIN SELECT RAISE(ABORT,'source lifecycle history is immutable'); END;

CREATE TABLE source_learning_jobs (
    job_id TEXT PRIMARY KEY,
    source_id TEXT NOT NULL REFERENCES sources(source_id) ON DELETE RESTRICT,
    revision_id TEXT NOT NULL REFERENCES source_revisions(revision_id) ON DELETE RESTRICT,
    generation INTEGER NOT NULL CHECK(generation > 0),
    compiler_version TEXT NOT NULL CHECK(compiler_version = 'moon-source-extract-v1'),
    status TEXT NOT NULL CHECK(status IN ('queued','leased','review','complete','dead','cancelled')),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK(attempts BETWEEN 0 AND 3),
    attempt_token TEXT,
    lease_until_ms INTEGER,
    not_before_ms INTEGER NOT NULL DEFAULT 0,
    last_error TEXT CHECK(last_error IN ('model_failed','invalid_response','interrupted','lease_expired','source_changed','withdrawn','deleted')),
    proposals_json TEXT,
    result_hash TEXT,
    outcome_json TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    CHECK((status = 'leased') = (lease_until_ms IS NOT NULL)),
    CHECK(status <> 'leased' OR attempt_token IS NOT NULL),
    CHECK((proposals_json IS NULL) = (result_hash IS NULL)),
    UNIQUE(source_id, revision_id, generation, compiler_version)
);
CREATE INDEX source_learning_jobs_ready ON source_learning_jobs(status,not_before_ms,created_at_ms,job_id);
-- Explicit eligibility recorded by milestone 1 becomes pending, never silently
-- processed. No service or model call is enabled by migration.
INSERT INTO source_learning_jobs(job_id,source_id,revision_id,generation,compiler_version,status,created_at_ms,updated_at_ms)
SELECT 'slj_' || lower(hex(randomblob(16))),s.source_id,r.revision_id,1,
       'moon-source-extract-v1','queued',r.captured_at_ms,r.captured_at_ms
FROM sources s JOIN source_revisions r ON r.source_id=s.source_id
WHERE r.learning_eligibility='eligible'
AND r.revision=(SELECT max(h.revision) FROM source_revisions h WHERE h.source_id=s.source_id);

CREATE TABLE source_claims (
    claim_id TEXT PRIMARY KEY,
    document_id INTEGER NOT NULL UNIQUE REFERENCES documents(id) ON DELETE RESTRICT,
    scope TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('author_statement','user_statement','generated_statement')),
    content TEXT NOT NULL CHECK(length(CAST(content AS BLOB)) BETWEEN 1 AND 8192),
    content_hash TEXT NOT NULL,
    identity_hash TEXT NOT NULL UNIQUE,
    valid_until_ms INTEGER CHECK(valid_until_ms > 0),
    created_at_ms INTEGER NOT NULL
);
CREATE INDEX source_claims_scope ON source_claims(scope,created_at_ms,claim_id);
CREATE TABLE source_claim_supports (
    claim_id TEXT NOT NULL REFERENCES source_claims(claim_id) ON DELETE RESTRICT,
    revision_id TEXT NOT NULL REFERENCES source_revisions(revision_id) ON DELETE RESTRICT,
    start_byte INTEGER NOT NULL CHECK(start_byte >= 0),
    end_byte INTEGER NOT NULL CHECK(end_byte > start_byte),
    start_line INTEGER NOT NULL CHECK(start_line >= 1),
    end_line INTEGER NOT NULL CHECK(end_line >= start_line),
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY(claim_id,revision_id,start_byte,end_byte)
);
CREATE INDEX source_claim_supports_revision ON source_claim_supports(revision_id,claim_id);
CREATE VIEW source_valid_supports AS
SELECT c.claim_id,c.document_id,s.source_id,s.scope,s.source_key,r.revision_id,
       x.start_byte,x.end_byte,x.start_line,x.end_line
FROM source_claim_supports x JOIN source_claims c ON c.claim_id=x.claim_id
JOIN source_revisions r ON r.revision_id=x.revision_id
JOIN sources s ON s.source_id=r.source_id
JOIN source_lifecycle l ON l.source_id=s.source_id
WHERE l.status='active' AND r.learning_eligibility='eligible' AND s.scope=c.scope
AND r.revision=(SELECT max(h.revision) FROM source_revisions h WHERE h.source_id=s.source_id);
CREATE VIEW source_valid_claims AS
SELECT DISTINCT c.claim_id,c.document_id FROM source_claims c
JOIN source_valid_supports v ON v.claim_id=c.claim_id
WHERE c.valid_until_ms IS NULL OR c.valid_until_ms > CAST(unixepoch('subsec') * 1000 AS INTEGER);

CREATE TRIGGER source_claims_immutable_update BEFORE UPDATE ON source_claims
BEGIN SELECT RAISE(ABORT,'source statement is immutable'); END;
CREATE TRIGGER source_claim_supports_immutable_update BEFORE UPDATE ON source_claim_supports
BEGIN SELECT RAISE(ABORT,'source support is immutable'); END;
-- Explicit deletion leaves a non-content tombstone. Normal revision writes and
-- withdrawals still cannot erase evidence. The CLI verifies a dependency-aware
-- deletion plan before changing lifecycle state in the same transaction.
DROP TRIGGER source_revisions_immutable_delete;
CREATE TRIGGER source_revisions_immutable_delete BEFORE DELETE ON source_revisions
WHEN NOT EXISTS(SELECT 1 FROM source_lifecycle l WHERE l.source_id=OLD.source_id AND l.status='deleted')
BEGIN SELECT RAISE(ABORT,'source revision is immutable'); END;
