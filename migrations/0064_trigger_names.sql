-- compat: previous -- one additive nullable column (`triggers.name`) plus
-- one additive unique index; an old release simply never queries either,
-- no existing row's shape changes (PRD-mcphost-migration-safety
-- requirement 4). `name` starts NULL for every pre-existing row --
-- `backfill_trigger_names_sync` (db.rs), not this file, fills it in,
-- since collision suffixing (requirement 1: `<kind>:<tool>`, `-2`, `-3`...)
-- needs per-row Rust logic a single SQL statement can't express. The
-- unique index is created here, immediately, rather than after the
-- backfill -- SQLite treats every `NULL` in a UNIQUE index as distinct
-- from every other `NULL`, so creating it now while every pre-existing row
-- is still unbackfilled can never fail, and it's already enforcing
-- uniqueness for every row the backfill assigns a name to afterward.
-- mcphost 0064_trigger_names: PRD-mcphost-trigger-set-idempotent P0
-- requirement 1. (Renumbered from this PRD's own 0059, then 0062, during
-- rebase: mcphost-url-bound-tenants claimed 0059 first, then
-- mcphost-one-next-tool claimed 0060, then mcphost-chain-host-steps claimed
-- 0061, then mcphost-chain-run-lineage claimed 0062, then
-- mcphost-table-concept-graph claimed 0063, all landing on main ahead of
-- this branch.)
ALTER TABLE triggers ADD COLUMN name TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS idx_triggers_tenant_name ON triggers(tenant_id, name);
