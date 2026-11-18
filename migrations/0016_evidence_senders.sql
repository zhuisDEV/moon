-- Existing admissions do not identify the author, even in a one-reader room.
ALTER TABLE privacy_admissions ADD COLUMN sender TEXT;
CREATE INDEX privacy_admissions_sender ON privacy_admissions(sender) WHERE sender IS NOT NULL;
CREATE TRIGGER privacy_admissions_require_sender BEFORE INSERT ON privacy_admissions
WHEN NEW.sender IS NULL OR NOT EXISTS (
    SELECT 1 FROM json_each(NEW.audience_json) WHERE type='text' AND value=NEW.sender
)
BEGIN SELECT RAISE(ABORT,'sender must be a verified reader'); END;
CREATE TRIGGER privacy_admissions_immutable BEFORE UPDATE ON privacy_admissions
BEGIN SELECT RAISE(ABORT,'admission is immutable'); END;
CREATE TRIGGER privacy_admissions_retained BEFORE DELETE ON privacy_admissions
BEGIN SELECT RAISE(ABORT,'admission is immutable'); END;

-- Keep caller-supplied evidence metadata separate from authenticated attribution.
CREATE TABLE evidence_admissions (
    evidence_id INTEGER PRIMARY KEY REFERENCES evidence_sessions(id) ON DELETE RESTRICT,
    admission_key TEXT NOT NULL UNIQUE REFERENCES privacy_admissions(admission_key) ON DELETE RESTRICT
);
CREATE TRIGGER evidence_admissions_require_audience BEFORE INSERT ON evidence_admissions
WHEN NOT EXISTS (
    SELECT 1 FROM evidence_sessions e JOIN documents d ON d.id=e.document_id
    JOIN privacy_audiences a ON a.scope=d.scope AND a.kind='private'
    JOIN privacy_admissions t ON t.audience_json=a.readers_json AND t.sender IS NOT NULL
    WHERE e.id=NEW.evidence_id AND t.admission_key=NEW.admission_key
)
BEGIN SELECT RAISE(ABORT,'evidence audience differs from its admission'); END;
CREATE TRIGGER evidence_admissions_immutable BEFORE UPDATE ON evidence_admissions
BEGIN SELECT RAISE(ABORT,'evidence attribution is immutable'); END;
CREATE TRIGGER evidence_admissions_retained BEFORE DELETE ON evidence_admissions
BEGIN SELECT RAISE(ABORT,'evidence attribution is immutable'); END;
