-- compat: previous -- additive columns with default NULL on an existing
-- table; an old release simply never queries them (PRD-mcphost-migration-safety
-- requirement 4).
-- mcphost 0048_runs_end_user_subject: PRD-mcphost-runs-end-user-subject P0
-- requirement 1.
--
-- `runs` gains the end user a run ran as, when any: `end_user_subject`/
-- `end_user_issuer` (issuer only ever set for `end_user_method = 'oauth'`)
-- and `end_user_method` (`'oauth'` or `'assertion'`) -- same three-column
-- shape migration 0042 already gave `calls`. Indexed alongside `tenant_id`
-- for `host.runs.list {end_user_subject}`-style per-tenant-per-end-user
-- lookups (AC2/AC6).
ALTER TABLE runs ADD COLUMN end_user_subject TEXT;
ALTER TABLE runs ADD COLUMN end_user_issuer TEXT;
ALTER TABLE runs ADD COLUMN end_user_method TEXT;

CREATE INDEX IF NOT EXISTS idx_runs_tenant_end_user ON runs(tenant_id, end_user_subject);
