-- Schema 17 cannot establish who had access when a historical reply was sent.
-- Keep those rows for local audit without granting them new recall access.
ALTER TABLE private_turn_history ADD COLUMN reply_readers_json TEXT
    CHECK(reply_readers_json IS NULL OR (json_valid(reply_readers_json) AND json_type(reply_readers_json)='array'));
ALTER TABLE private_turn_history ADD COLUMN reply_owners_json TEXT
    CHECK(reply_owners_json IS NULL OR (json_valid(reply_owners_json) AND json_type(reply_owners_json)='array'));

ALTER TABLE private_turn_history ADD COLUMN reply_delivered_at_ms INTEGER;

CREATE TRIGGER private_reply_acl_immutable BEFORE UPDATE ON private_turn_history
WHEN OLD.discord_message_id IS NOT NULL AND (
     NEW.discord_message_id IS NOT OLD.discord_message_id OR
     NEW.reply_delivered_at_ms IS NOT OLD.reply_delivered_at_ms OR
     NEW.reply_readers_json IS NOT OLD.reply_readers_json OR
     NEW.reply_owners_json IS NOT OLD.reply_owners_json)
BEGIN SELECT RAISE(ABORT,'delivered reply ownership is immutable'); END;

-- NULL marks admissions made under older rules. Never reclassify them from
-- today's room permissions. This is the time the host snapshot was recorded.
ALTER TABLE privacy_admissions ADD COLUMN room_access_at_ms INTEGER
    CHECK(room_access_at_ms IS NULL OR room_access_at_ms>0);
