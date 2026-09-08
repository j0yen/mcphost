-- compat: previous -- additive columns; a rolled-back binary that never
-- reads them runs unmodified against the migrated schema
-- (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0010_tenant_attribution: PRD-mcphost-tenant-attribution P0
-- requirements 1, 2, 5.
--
-- `source_class` (`'loopback'|'fleet'|'external'`) is this host's own
-- derived read of where a signup came from -- see
-- `state::classify_source_class`. It is independent of, but consulted
-- alongside, `tenants.synthetic` (migration 0008), which still carries the
-- harness's own free-form label when it sends one.
--
-- `client_name`/`client_version` come from the MCP `initialize` request's
-- `clientInfo` (requirement 2), captured at signup or -- if signup somehow
-- preceded capture -- on the tenant's first authenticated call after.
--
-- `classified_by` is `NULL` for every live signup (classified inline, at
-- signup time) and `'backfill'` only for the one-shot reclassification
-- this migration also runs, below, against every tenant that predates
-- this column (requirement 5 / AC3).
--
-- `created_unix` mirrors `calls.started_unix`'s plain-epoch-seconds shape
-- for `created_at` (an opaque `"unix:<secs>.<nanos>"` string to every
-- other reader) so `mcphost funnel`'s signup-to-first-call latency and
-- `--since` filter don't need to parse it.
ALTER TABLE tenants ADD COLUMN source_class TEXT;
ALTER TABLE tenants ADD COLUMN client_name TEXT;
ALTER TABLE tenants ADD COLUMN client_version TEXT;
ALTER TABLE tenants ADD COLUMN classified_by TEXT;
ALTER TABLE tenants ADD COLUMN created_unix INTEGER;

-- Requirement 2: `signup_events.user_agent`, alongside the existing
-- `source_ip`/`synthetic` (migration 0008) -- the durable per-signup
-- ledger, not the tenant's current state.
ALTER TABLE signup_events ADD COLUMN user_agent TEXT;
