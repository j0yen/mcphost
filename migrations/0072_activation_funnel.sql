-- compat: previous -- seven additive nullable columns on `tenants` plus
-- one index; an old release simply never queries any of them, no existing
-- statement's result set changes (PRD-mcphost-migration-safety
-- requirement 4).
-- mcphost 0072_activation_funnel: PRD-mcphost-activation-funnel
-- requirement 1.
--
-- Six activation stamps, each written once (`UPDATE ... SET col =
-- COALESCE(col, ?)`) on the hot path that first makes it true:
-- `first_call_unix` (any authenticated call), `first_publish_unix`
-- (`host.tool_publish`), `first_own_call_unix` (a call to a tool this
-- tenant itself published), `second_session_unix` (a reconnect on a
-- different session, >= 10 minutes after `created_unix`), `claimed_unix`
-- (alias of `owner_verified_at`), `paid_unix` (from `plan_since` once
-- `plan != 'free'`). `created_session_id` is the seventh column -- the
-- session id `handler::bind_session_to_created_tenant` recorded at birth,
-- which `second_session_unix`'s own write compares every later session
-- against (technical considerations: "store the session id recorded at
-- creation").
--
-- `claimed_unix`/`paid_unix` are backfilled from `owner_verified_at`/
-- `plan_since` for any tenant that predates this column -- see
-- `Db::backfill_activation_claims_and_plans`, called once from this
-- migration's own idempotency gate (`Db::migrate_0072_activation_funnel`).
ALTER TABLE tenants ADD COLUMN first_call_unix INTEGER;
ALTER TABLE tenants ADD COLUMN first_publish_unix INTEGER;
ALTER TABLE tenants ADD COLUMN first_own_call_unix INTEGER;
ALTER TABLE tenants ADD COLUMN second_session_unix INTEGER;
ALTER TABLE tenants ADD COLUMN claimed_unix INTEGER;
ALTER TABLE tenants ADD COLUMN paid_unix INTEGER;
ALTER TABLE tenants ADD COLUMN created_session_id TEXT;

-- `admin.funnel`/`/healthz`'s `funnel_7d` both filter by `source_class`
-- and window on `created_unix`; this composite index serves both without
-- a full table scan as the `tenants` table grows.
CREATE INDEX IF NOT EXISTS idx_tenants_funnel ON tenants(source_class, created_unix);
