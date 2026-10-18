-- compat: previous -- additive nullable columns on `tools`; an old release never
-- queries them, and the backfill only fills the new column.
-- mcphost 0123_share_scopes: PRD-mcphost-scoped-tool-share R2/R5/R6.
--
-- `share_scopes_json` is the JSON array of `ShareScope` names a share grants
-- (NULL while private). Every tool already shared when this lands is
-- backfilled to `["call","read_source"]` so no current grantee loses the spec
-- readback it had. `share_rate_per_hour` is the optional per-caller hourly
-- cap. `unshared_unix` is when the last unshare happened (the `revoked_at` of
-- `share_revoked`). `forked_from` records `<owner>.<tool>` on a fork.
ALTER TABLE tools ADD COLUMN share_scopes_json TEXT;
ALTER TABLE tools ADD COLUMN share_rate_per_hour INTEGER;
ALTER TABLE tools ADD COLUMN unshared_unix INTEGER;
ALTER TABLE tools ADD COLUMN forked_from TEXT;
UPDATE tools SET share_scopes_json = '["call","read_source"]' WHERE visibility != 'private';
