-- compat: previous -- one data-only UPDATE against the existing `runs`
-- table; no column, index or table shape changes (PRD-mcphost-migration-
-- safety requirement 4).
-- mcphost 0049_shared_call_run_scope: PRD-mcphost-shared-call-run-scope P1
-- requirement 6 / AC7.
--
-- Before this PRD, a synchronous cross-tenant call's `runs` row was
-- written under the OWNER's tenant id (`tenant_id = owner.id`,
-- `caller_tenant_id = caller.id`, `tool_name` the bare local name) -- the
-- bug requirement 1 fixes for every new row. This backfills the rows that
-- shape already produced, moving each to the CALLER the same way a new
-- row now lands: `tenant_id` becomes the old `caller_tenant_id`,
-- `tool_name` is qualified `<owner_ns>.<local>` using the OLD `tenant_id`
-- (the owner) to look up its namespace, and `caller_tenant_id` is cleared
-- to NULL -- matching `runs::enqueue_shared`'s own async-row convention,
-- which never sets `caller_tenant_id` either.
--
-- Idempotent by construction, not by an `IF NOT EXISTS` guard: once a row
-- is rewritten its `caller_tenant_id` is NULL, so it can never match this
-- statement's WHERE clause again -- a rerun (`migrate()` runs at every
-- `serve` start) touches zero rows the second time. The WHERE clause is
-- also the exact shape only the pre-fix synchronous cross-tenant path
-- could ever produce: an async run's own `enqueue_shared` insert already
-- writes `caller_tenant_id = NULL`, and an own-tenant call's
-- `caller_tenant_id` is always NULL too, so neither is touched.
UPDATE runs
SET
    tool_name = (SELECT t.namespace FROM tenants t WHERE t.id = runs.tenant_id) || '.' || runs.tool_name,
    tenant_id = runs.caller_tenant_id,
    caller_tenant_id = NULL
WHERE trigger = 'call'
  AND caller_tenant_id IS NOT NULL
  AND caller_tenant_id != tenant_id;
