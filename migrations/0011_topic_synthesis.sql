-- Topics are reviewed derived references, never personal memories. Dependencies
-- are flattened at admission, so every read can validate the entire bounded DAG.
CREATE TABLE topics (
 topic_id TEXT PRIMARY KEY,
 scope TEXT NOT NULL,
 topic_key TEXT NOT NULL,
 title TEXT NOT NULL,
 definition_json TEXT NOT NULL,
 generation INTEGER NOT NULL CHECK(generation>0),
 created_at_ms INTEGER NOT NULL,
 updated_at_ms INTEGER NOT NULL,
 UNIQUE(scope,topic_key)
);
CREATE TABLE topic_aliases (
 scope TEXT NOT NULL, alias TEXT NOT NULL,
 topic_id TEXT NOT NULL REFERENCES topics(topic_id) ON DELETE CASCADE,
 PRIMARY KEY(scope,alias)
);
CREATE TABLE topic_inputs (
 topic_id TEXT NOT NULL REFERENCES topics(topic_id) ON DELETE CASCADE,
 kind TEXT NOT NULL CHECK(kind IN ('claim','topic')), input_id TEXT NOT NULL,
 PRIMARY KEY(topic_id,kind,input_id)
);
CREATE INDEX topic_inputs_reverse ON topic_inputs(kind,input_id,topic_id);
CREATE TABLE topic_jobs (
 job_id TEXT PRIMARY KEY,
 topic_id TEXT NOT NULL REFERENCES topics(topic_id) ON DELETE RESTRICT,
 generation INTEGER NOT NULL,
 base_revision_id TEXT,
 input_fingerprint TEXT,
 snapshot_json TEXT,
 status TEXT NOT NULL CHECK(status IN ('queued','leased','review','complete','dead','cancelled')),
 attempts INTEGER NOT NULL DEFAULT 0 CHECK(attempts BETWEEN 0 AND 3),
 attempt_token TEXT,
 lease_until_ms INTEGER,
 not_before_ms INTEGER NOT NULL DEFAULT 0,
 last_scan_ms INTEGER NOT NULL DEFAULT 0,
 last_error TEXT CHECK(last_error IN ('model_failed','invalid_response','interrupted','lease_expired','inputs_changed','deleted','duplicate')),
 proposals_json TEXT, result_hash TEXT, outcome_json TEXT,
 created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL,
 CHECK((status='leased')=(lease_until_ms IS NOT NULL)),
 CHECK(status<>'leased' OR (attempt_token IS NOT NULL AND snapshot_json IS NOT NULL)),
 CHECK((snapshot_json IS NULL)=(input_fingerprint IS NULL)),
 CHECK((proposals_json IS NULL)=(result_hash IS NULL)),
 UNIQUE(topic_id,generation,input_fingerprint)
);
CREATE INDEX topic_jobs_ready ON topic_jobs(status,not_before_ms,created_at_ms,job_id);
CREATE TABLE topic_dependencies (
 job_id TEXT NOT NULL REFERENCES topic_jobs(job_id) ON DELETE CASCADE,
 kind TEXT NOT NULL CHECK(kind IN ('claim','source','topic')),
 input_id TEXT NOT NULL, revision_id TEXT NOT NULL,
 generation INTEGER NOT NULL,
 expires_at_ms INTEGER,
 PRIMARY KEY(job_id,kind,input_id,revision_id)
);
CREATE INDEX topic_dependencies_reverse ON topic_dependencies(kind,input_id,job_id);
CREATE TRIGGER topic_dependencies_immutable BEFORE UPDATE ON topic_dependencies
BEGIN SELECT RAISE(ABORT,'topic dependency is immutable'); END;
CREATE TABLE topic_revisions (
 revision_id TEXT PRIMARY KEY,
 topic_id TEXT NOT NULL REFERENCES topics(topic_id) ON DELETE RESTRICT,
 revision INTEGER NOT NULL CHECK(revision>0),
 job_id TEXT NOT NULL UNIQUE REFERENCES topic_jobs(job_id) ON DELETE RESTRICT,
 supersedes_revision_id TEXT,
 document_id INTEGER UNIQUE REFERENCES documents(id) ON DELETE RESTRICT,
 content_json TEXT,
 redacted INTEGER NOT NULL DEFAULT 0 CHECK(redacted IN (0,1)),
 created_at_ms INTEGER NOT NULL,
 CHECK((redacted=1)=(content_json IS NULL)),
 CHECK((redacted=1)=(document_id IS NULL)),
 UNIQUE(topic_id,revision)
);
CREATE TRIGGER topic_revision_immutable BEFORE UPDATE ON topic_revisions
WHEN NOT (OLD.redacted=0 AND NEW.redacted=1 AND NEW.content_json IS NULL AND NEW.document_id IS NULL
 AND NEW.revision_id=OLD.revision_id AND NEW.topic_id=OLD.topic_id AND NEW.revision=OLD.revision
 AND NEW.job_id=OLD.job_id AND NEW.supersedes_revision_id IS OLD.supersedes_revision_id AND NEW.created_at_ms=OLD.created_at_ms)
BEGIN SELECT RAISE(ABORT,'topic revision is immutable except explicit source deletion'); END;
CREATE TRIGGER topic_revision_no_delete BEFORE DELETE ON topic_revisions
BEGIN SELECT RAISE(ABORT,'topic revision tombstones are retained'); END;

CREATE VIEW topic_valid_jobs AS
SELECT j.job_id FROM topic_jobs j JOIN topics t ON t.topic_id=j.topic_id
WHERE j.snapshot_json IS NOT NULL AND j.generation=t.generation
AND EXISTS(SELECT 1 FROM topic_dependencies d WHERE d.job_id=j.job_id)
AND NOT EXISTS (
 SELECT 1 FROM topic_dependencies d WHERE d.job_id=j.job_id AND (
 (d.expires_at_ms IS NOT NULL AND d.expires_at_ms<=CAST(unixepoch('subsec')*1000 AS INTEGER))
 OR (d.kind='claim' AND NOT EXISTS (
  SELECT 1 FROM source_claims c JOIN source_valid_claims v ON v.claim_id=c.claim_id
  WHERE c.claim_id=d.input_id AND c.scope=t.scope AND c.content_hash=d.revision_id))
 OR (d.kind='source' AND NOT EXISTS (
  SELECT 1 FROM sources s JOIN source_lifecycle l ON l.source_id=s.source_id
  JOIN source_revisions r ON r.source_id=s.source_id
  WHERE s.source_id=d.input_id AND s.scope=t.scope AND l.status='active' AND l.generation=d.generation
  AND r.revision_id=d.revision_id AND r.learning_eligibility='eligible'
  AND r.revision=(SELECT max(h.revision) FROM source_revisions h WHERE h.source_id=s.source_id)))
 OR (d.kind='topic' AND NOT EXISTS (
  SELECT 1 FROM topics p JOIN topic_revisions r ON r.topic_id=p.topic_id
  WHERE p.topic_id=d.input_id AND p.scope=t.scope AND p.generation=d.generation
  AND r.revision_id=d.revision_id AND r.redacted=0
  AND r.revision=(SELECT max(h.revision) FROM topic_revisions h WHERE h.topic_id=p.topic_id)))
 ));
CREATE VIEW topic_valid_documents AS
SELECT r.document_id,r.revision_id,r.topic_id FROM topic_revisions r
JOIN topic_jobs j ON j.job_id=r.job_id JOIN topic_valid_jobs v ON v.job_id=j.job_id
WHERE r.redacted=0 AND j.status='complete'
AND r.revision=(SELECT max(h.revision) FROM topic_revisions h WHERE h.topic_id=r.topic_id);
