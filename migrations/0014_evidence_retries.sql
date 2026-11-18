-- A deferral remains pending; the ordinary processed ledger is untouched.
CREATE TABLE learning_evidence_retries (
    evidence_session_id INTEGER PRIMARY KEY REFERENCES evidence_sessions(id) ON DELETE RESTRICT,
    attempts INTEGER NOT NULL CHECK (attempts IN (1, 2)),
    last_occurrence TEXT NOT NULL,
    rule_version TEXT NOT NULL,
    support_after_id INTEGER NOT NULL,
    support_id INTEGER REFERENCES evidence_sessions(id) ON DELETE RESTRICT
);
CREATE INDEX learning_retry_unversioned ON learning_evidence_retries(rule_version)
    WHERE rule_version = '';

-- Index only deferred turns. Ingestion looks up new same-scope matches once;
-- an idle scheduler never compares the bodies of parked conversations.
CREATE TABLE learning_retry_terms (
    scope TEXT NOT NULL,
    term TEXT NOT NULL,
    evidence_session_id INTEGER NOT NULL REFERENCES learning_evidence_retries(evidence_session_id) ON DELETE CASCADE,
    PRIMARY KEY(scope, term, evidence_session_id)
) WITHOUT ROWID;
CREATE INDEX learning_retry_terms_session ON learning_retry_terms(evidence_session_id);

-- Only explicit committed deferrals in old runs are attributable to a source.
-- Other pending turns get their normal first review.
WITH history AS (
    SELECT e.id, r.run_key, r.completed_at_ms,
           row_number() OVER (PARTITION BY e.id ORDER BY r.completed_at_ms DESC, r.rowid DESC) AS latest,
           count(*) OVER (PARTITION BY e.id) AS attempts
    FROM learning_runs r JOIN json_each(r.outcome_json, '$.deferred_session_ids') j
    JOIN evidence_sessions e ON e.session_id = j.value
    LEFT JOIN learning_processed_evidence p ON p.evidence_session_id = e.id
    WHERE r.status = 'committed' AND p.evidence_session_id IS NULL
)
INSERT INTO learning_evidence_retries(evidence_session_id, attempts, last_occurrence, rule_version, support_after_id)
SELECT id, min(2, attempts),
       CASE WHEN run_key GLOB 'l2:????-??-??:*' THEN substr(run_key, 4, 10) ELSE run_key END, '',
       max(id, coalesce((SELECT max(new.id) FROM evidence_sessions new
                         WHERE new.recorded_at_ms <= history.completed_at_ms), 0))
FROM history WHERE latest = 1;
