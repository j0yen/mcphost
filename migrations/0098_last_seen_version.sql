-- compat: previous -- one additive nullable column on an existing table; an
-- old release simply never queries it
-- (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0095_last_seen_version: PRD-mcphost-host-changelog-since R4.
--
-- The host version this tenant last called, written with `last_seen_unix` on
-- every authenticated call; `host.changelog {}` defaults its `since` to the
-- value the tenant carried BEFORE the call. NULL until the first call after
-- this migration.
ALTER TABLE tenants ADD COLUMN last_seen_version TEXT;
