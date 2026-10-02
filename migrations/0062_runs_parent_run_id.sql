-- compat: previous -- three additive nullable columns on an existing
-- table; an old release simply never queries them
-- (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0062_runs_parent_run_id: PRD-mcphost-chain-run-lineage
-- requirement 4 / P1 requirement 8. (Renumbered from this PRD's own 0057
-- during rebase: mcphost-chart-in-a-minute, mcphost-lineage-blast-radius,
-- mcphost-url-bound-tenants, mcphost-one-next-tool, and
-- mcphost-chain-host-steps claimed 0057 through 0061 first, landing on
-- main ahead of this branch.)
--
-- A chain call now writes one `runs` row per executed step
-- (`Db::insert_composed_run`, `trigger = 'composition'`) instead of only
-- the chain's own `steps` trace -- `parent_run_id` is the column
-- `host.runs.get`'s "one level" child inlining and `host.runs.list
-- {parent_run_id}` filter both key on. `step_no`/`parent_tool` (P1
-- requirement 8, AC11) let a failed child be found by
-- `host.runs.list(status="failed", trigger="composition")` without a
-- second query back to the parent. Same additive-column shape migration
-- 0048's `end_user_subject`/`end_user_issuer`/`end_user_method` used.
ALTER TABLE runs ADD COLUMN parent_run_id TEXT;
ALTER TABLE runs ADD COLUMN step_no INTEGER;
ALTER TABLE runs ADD COLUMN parent_tool TEXT;

CREATE INDEX IF NOT EXISTS idx_runs_tenant_parent ON runs(tenant_id, parent_run_id);
