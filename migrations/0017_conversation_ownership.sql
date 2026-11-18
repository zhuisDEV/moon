-- Assignment applies only to the global records frozen by migration 15.
-- It grants private recall of saved memory text without granting the legacy
-- evidence documents or backfilling actors in historical sessions.
CREATE TABLE legacy_collection_owner (
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    owner TEXT NOT NULL,
    scope TEXT NOT NULL REFERENCES privacy_audiences(scope),
    assigned_at_ms INTEGER NOT NULL
);
CREATE TRIGGER legacy_owner_requires_single_reader BEFORE INSERT ON legacy_collection_owner
WHEN NOT EXISTS (SELECT 1 FROM privacy_audiences a WHERE a.scope=NEW.scope
    AND a.kind='private' AND json_array_length(a.readers_json)=1
    AND json_extract(a.readers_json,'$[0]')=NEW.owner)
BEGIN SELECT RAISE(ABORT,'legacy collection requires its owner private audience'); END;
-- Enforce the same owner on the older per-record recovery route in either order.
CREATE TRIGGER legacy_owner_recoveries_consistent BEFORE INSERT ON legacy_collection_owner
WHEN EXISTS (SELECT 1 FROM privacy_recoveries WHERE scope<>NEW.scope)
BEGIN SELECT RAISE(ABORT,'existing legacy recovery belongs to another audience'); END;
CREATE TRIGGER privacy_recovery_owner_scope BEFORE INSERT ON privacy_recoveries
WHEN EXISTS (SELECT 1 FROM legacy_collection_owner WHERE scope<>NEW.scope)
BEGIN SELECT RAISE(ABORT,'legacy recovery must retain the collection owner'); END;
CREATE TRIGGER legacy_owner_immutable BEFORE UPDATE ON legacy_collection_owner
BEGIN SELECT RAISE(ABORT,'legacy ownership is immutable'); END;
CREATE TRIGGER legacy_owner_retained BEFORE DELETE ON legacy_collection_owner
BEGIN SELECT RAISE(ABORT,'legacy ownership is immutable'); END;
CREATE TRIGGER privacy_assigned_legacy AFTER INSERT ON legacy_collection_owner
BEGIN UPDATE privacy_revision SET revision=revision+1 WHERE id=1; END;

CREATE VIEW owned_legacy_memories AS
SELECT m.document_id, o.scope
FROM legacy_collection_owner o CROSS JOIN privacy_legacy_documents l
JOIN memory_items m ON m.document_id=l.document_id;

-- Only chat recall uses the additional branch. Learning and source selection
-- continue to require their own authenticated evidence.
CREATE VIEW chat_recallable_memories AS
SELECT document_id,scope FROM audience_recallable_memories
UNION ALL
SELECT o.document_id,o.scope FROM owned_legacy_memories o
JOIN documents d ON d.id=o.document_id
JOIN memory_items m ON m.document_id=o.document_id
WHERE d.active=1 AND m.superseded_by IS NULL
AND (m.valid_until_ms IS NULL OR m.valid_until_ms>CAST(unixepoch('subsec')*1000 AS INTEGER));

-- Older admissions captured potential readers but did not attest who was present.
ALTER TABLE privacy_admissions ADD COLUMN owners_json TEXT
    CHECK(owners_json IS NULL OR (json_valid(owners_json) AND json_type(owners_json)='array'));

CREATE TRIGGER privacy_admissions_require_owners BEFORE INSERT ON privacy_admissions
WHEN NEW.owners_json IS NULL
    OR NOT EXISTS (SELECT 1 FROM json_each(NEW.owners_json) WHERE value=NEW.sender)
    OR EXISTS (SELECT value FROM json_each(NEW.owners_json)
               EXCEPT SELECT value FROM json_each(NEW.audience_json))
BEGIN SELECT RAISE(ABORT,'admission owners must include sender and be verified readers'); END;

DROP TRIGGER evidence_admissions_require_audience;
CREATE TRIGGER evidence_admissions_require_audience BEFORE INSERT ON evidence_admissions
WHEN NOT EXISTS (
    SELECT 1 FROM evidence_sessions e JOIN documents d ON d.id=e.document_id
    JOIN privacy_audiences a ON a.scope=d.scope AND a.kind='private'
    JOIN privacy_admissions t ON t.owners_json=a.readers_json
    WHERE e.id=NEW.evidence_id AND t.admission_key=NEW.admission_key
)
BEGIN SELECT RAISE(ABORT,'evidence scope differs from admission owners'); END;


-- This records only verified activity observed for an admitted message. A replay
-- never makes the speaker newly active. The route is account and room specific.
CREATE TABLE admission_room_activity (
    admission_key TEXT PRIMARY KEY REFERENCES privacy_admissions(admission_key),
    room_key TEXT NOT NULL,
    event_ms INTEGER NOT NULL CHECK(event_ms > 0)
);
CREATE INDEX admission_room_activity_window ON admission_room_activity(room_key,event_ms);

-- Keep a digest for the first of two independently ordered events. Retain text
-- only after identical accepted and delivered final text are both proven.
CREATE TABLE private_turn_history (
    admission_key TEXT NOT NULL REFERENCES privacy_admissions(admission_key),
    reply_hash TEXT NOT NULL,
    accepted INTEGER NOT NULL CHECK(accepted IN (0,1)),
    discord_message_id TEXT UNIQUE,
    reply TEXT,
    PRIMARY KEY(admission_key,reply_hash),
    CHECK((reply IS NOT NULL) = (accepted=1 AND discord_message_id IS NOT NULL))
);
CREATE TRIGGER history_revision_insert AFTER INSERT ON private_turn_history
BEGIN UPDATE privacy_revision SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER history_revision_update AFTER UPDATE ON private_turn_history
BEGIN UPDATE privacy_revision SET revision=revision+1 WHERE id=1; END;
