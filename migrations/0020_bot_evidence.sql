-- An externally verified bot may be the source of a private message, but only
-- its human owner can recall it. Keep human admissions and old rows unchanged.
ALTER TABLE privacy_admissions ADD COLUMN sender_kind TEXT NOT NULL DEFAULT 'human'
    CHECK(sender_kind IN ('human','bot'));

-- Once an existing Discord principal owns cross-channel evidence, losing or
-- changing the profile configuration must never restore raw Discord access.
CREATE TABLE master_discord_principals (
    principal TEXT PRIMARY KEY NOT NULL CHECK (
        substr(principal,1,8)='discord:' AND length(principal) BETWEEN 23 AND 28
        AND substr(principal,9,1) BETWEEN '1' AND '9'
        AND substr(principal,9) NOT GLOB '*[^0-9]*'
    )
);
CREATE TRIGGER master_discord_principal_immutable BEFORE UPDATE ON master_discord_principals
BEGIN SELECT RAISE(ABORT,'master Discord principal is immutable'); END;
CREATE TRIGGER master_discord_principal_retained BEFORE DELETE ON master_discord_principals
BEGIN SELECT RAISE(ABORT,'master Discord principal is retained'); END;
CREATE TRIGGER privacy_new_master_discord_principal AFTER INSERT ON master_discord_principals
BEGIN UPDATE privacy_revision SET revision=revision+1 WHERE id=1; END;

DROP TRIGGER privacy_admissions_require_owners;
CREATE TRIGGER privacy_admissions_require_owners BEFORE INSERT ON privacy_admissions
WHEN NEW.owners_json IS NULL
    OR EXISTS (SELECT 1 FROM json_each(NEW.owners_json) WHERE value GLOB 'bot:*')
    OR EXISTS (SELECT value FROM json_each(NEW.owners_json)
               EXCEPT SELECT value FROM json_each(NEW.audience_json))
    OR (NEW.sender_kind='human' AND
        (NEW.sender GLOB 'bot:*' OR NOT EXISTS
         (SELECT 1 FROM json_each(NEW.owners_json) WHERE value=NEW.sender)))
    OR (NEW.sender_kind='bot' AND
        (NEW.sender NOT GLOB 'bot:discord:*' OR json_array_length(NEW.owners_json)<>1))
BEGIN SELECT RAISE(ABORT,'admission must have verified human owners'); END;

CREATE TRIGGER private_audiences_require_human_owners BEFORE INSERT ON privacy_audiences
WHEN NEW.kind='private' AND EXISTS (SELECT 1 FROM json_each(NEW.readers_json) WHERE value GLOB 'bot:*')
BEGIN SELECT RAISE(ABORT,'bots cannot own private memories'); END;

DROP TRIGGER speaker_scope_requires_owner;
CREATE TRIGGER speaker_scope_requires_admission BEFORE INSERT ON speaker_memory_scopes
WHEN NOT EXISTS (
    SELECT 1 FROM privacy_audiences a JOIN privacy_admissions t ON t.owners_json=a.readers_json
    WHERE a.scope=NEW.audience_scope AND a.kind='private' AND t.sender=NEW.sender
    AND (t.sender_kind='bot' OR EXISTS (SELECT 1 FROM json_each(t.owners_json) WHERE value=t.sender))
)
BEGIN SELECT RAISE(ABORT,'speaker scope must match an admitted source and its owners'); END;

CREATE TRIGGER bot_evidence_requires_role BEFORE INSERT ON evidence_admissions
WHEN EXISTS (SELECT 1 FROM privacy_admissions WHERE admission_key=NEW.admission_key AND sender_kind='bot')
    AND NOT EXISTS (
        SELECT 1 FROM evidence_sessions e JOIN documents d ON d.id=e.document_id
        WHERE e.id=NEW.evidence_id AND substr(d.body,1,5)='Bot:'||char(10) AND length(d.body)>5
    )
BEGIN SELECT RAISE(ABORT,'external bot evidence must carry its bot role'); END;
