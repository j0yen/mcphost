-- compat: previous -- two wholly new tables (CREATE TABLE IF NOT EXISTS,
-- same precedent as migrations 0011/0014/0026/0044); an old release simply
-- never queries either, and nothing about its existing behavior changes
-- (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0043_shared_tool_caller_usage: PRD-mcphost-shared-tool-caller-usage
-- P0 requirement 3, P1 requirement 6.
--
-- Requirement 1's per-caller/per-end-user usage breakdown (AC1/AC2/AC5/AC8)
-- is served straight off `calls`, which already carries `caller_tenant_id`
-- (migration 0013_sharing) and `end_user_subject` (migration
-- 0042_end_user_identity) -- the real per-call event log this host has
-- always metered `host.usage` from. `meter_events` (migration 0007) is a
-- different table entirely: the Stripe billing ledger's own record of
-- already-aggregated call SPANS (first_call_id..last_call_id, a count),
-- never a per-call row, so it carries no tool/caller/end-user to break
-- down in the first place -- adding those columns there would be dead
-- weight with no reader. This migration only adds what's actually new.
--
-- `usage_daily(tenant_id, tool, caller_tenant_id, end_user_subject, day,
-- calls, errors)` (requirement 6, AC7): a nightly rollup so a 30-day
-- `host.usage`/`host.usage {by}` query answers from ~30-90 small rows
-- instead of scanning a million-row `calls` table. `caller_tenant_id`/
-- `end_user_subject` use the same "sentinel, not NULL" convention
-- migration 0042's `tenant_state_kv` rebuild already established for this
-- exact reason: SQLite's PRIMARY KEY treats two NULLs as distinct, which
-- would let a re-run of the rollup insert a second row for the same
-- tenant-wide/no-end-user group instead of updating the first one. `0` is
-- never a real tenant id (`tenants.id` is an `INTEGER PRIMARY KEY`
-- rowid alias, which SQLite starts at 1) and `''` is never a real end-user
-- subject (enduser identity's own migration 0042 comment makes the same
-- argument for `tenant_state_kv`), so both are safe "absent" sentinels.
CREATE TABLE IF NOT EXISTS usage_daily (
    tenant_id        INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    tool             TEXT NOT NULL,
    caller_tenant_id INTEGER NOT NULL DEFAULT 0,
    end_user_subject TEXT NOT NULL DEFAULT '',
    day              TEXT NOT NULL,
    calls            INTEGER NOT NULL,
    errors           INTEGER NOT NULL,
    PRIMARY KEY (tenant_id, tool, caller_tenant_id, end_user_subject, day)
);
CREATE INDEX IF NOT EXISTS idx_usage_daily_tenant_day ON usage_daily(tenant_id, day);

-- Requirement 3 (AC3): `host.share.caller_limit`'s own durable setting --
-- one row per (owner tool, caller tenant), enforced in `handler.rs` before
-- a cross-tenant call ever dispatches, the same "rejected before any
-- sandboxed work happens" shape `check_calls_quota` already uses for the
-- plan-wide `calls_per_day` knob this is a per-caller refinement of.
CREATE TABLE IF NOT EXISTS tool_caller_limits (
    tenant_id        INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    tool_name        TEXT NOT NULL,
    caller_tenant_id INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    calls_per_day    INTEGER NOT NULL,
    created_unix     INTEGER NOT NULL,
    updated_unix     INTEGER NOT NULL,
    PRIMARY KEY (tenant_id, tool_name, caller_tenant_id)
);
