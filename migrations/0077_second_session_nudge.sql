-- compat: previous -- three additive nullable/defaulted columns on
-- `tenants` plus one index; an old release simply never queries any of
-- them, no existing statement's result set changes (PRD-mcphost-
-- migration-safety requirement 4).
-- mcphost 0077_second_session_nudge: PRD-mcphost-second-session-nudge
-- requirement 1. Renumbered from 0073, then again to 0077 during this
-- rebase (run 428, 2026-10-06): mcphost-ownership-moment claimed 0073-0075
-- first, then mcphost-upgrade-moment claimed 0076, both landing on main
-- ahead of this branch.
--
-- `nudged_unix` is set once the daily sweep (`returns::sweep`) has
-- finished processing a tenant -- sent, recorded as unreachable
-- (`nudge_channel = 'none'`), or given up on after three failed sends
-- (`nudge_channel = 'email-abandoned'`). A failed send that still has
-- retries left (`nudge_channel = 'email-failed'`, `nudge_attempts` < 3)
-- deliberately leaves `nudged_unix` NULL, so the next day's sweep selects
-- the row again; `Db::returns_sweep_candidates`' own `WHERE` clause is the
-- one place that reads this distinction.
ALTER TABLE tenants ADD COLUMN nudged_unix INTEGER;
ALTER TABLE tenants ADD COLUMN nudge_channel TEXT;
ALTER TABLE tenants ADD COLUMN nudge_attempts INTEGER NOT NULL DEFAULT 0;

-- The sweep's own selection query filters on `nudge_channel`/
-- `first_call_unix` every day; this composite index keeps that a
-- bounded-by-`LIMIT` scan rather than a full table scan as `tenants` grows.
CREATE INDEX IF NOT EXISTS idx_tenants_nudge ON tenants(nudge_channel, first_call_unix);
