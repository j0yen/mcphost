-- compat: previous -- additive columns, null reproduces current behavior
-- exactly, consistent with every prior migration's "applies forward with
-- defaults" contract (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0008_synthetic: PRD-mcphost-synthetic-flag P0 requirement 1.
--
-- `tenants.synthetic` labels a tenant a test harness created (set at
-- signup from the `x-mcphost-synthetic` header) or the operator later
-- retro-tagged (`admin.tenant_set_synthetic` / `admin.tenants_set_synthetic`).
-- Free-form text, no enum/registry, so run ids compose
-- (`synthorg:<run_id>`); validated at the call sites, not by the schema.
-- Null means "unlabeled" -- today's undifferentiated behavior, unchanged.
ALTER TABLE tenants ADD COLUMN synthetic TEXT;

-- P2 requirement 8: `signup_events` also records the label a signup
-- carried, for after-the-fact auditing of unlabeled waves. Additive and
-- independent of the `tenants.synthetic` column above -- this is the
-- durable per-signup ledger, not the tenant's current state.
ALTER TABLE signup_events ADD COLUMN synthetic TEXT;
