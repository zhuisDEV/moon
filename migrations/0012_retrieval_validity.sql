-- Each claim already has a unique claim_id and document_id. An existence
-- check preserves the original DISTINCT join's semantics without forcing
-- joined topic/premise lookups to materialise every valid claim first.
-- Keep expiry and source lifecycle validation at read time: an active index
-- entry alone cannot establish that its evidence is still current.
DROP VIEW source_valid_claims;
CREATE VIEW source_valid_claims AS
SELECT c.claim_id,c.document_id
FROM source_claims c
WHERE (c.valid_until_ms IS NULL OR c.valid_until_ms > CAST(unixepoch('subsec') * 1000 AS INTEGER))
AND EXISTS(SELECT 1 FROM source_valid_supports v WHERE v.claim_id=c.claim_id);
