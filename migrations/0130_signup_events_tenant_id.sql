-- compat: previous -- one nullable additive column on `signup_events`; an old release
-- never selects it, and every existing row reads NULL (no backfill).
-- mcphost 0130_signup_events_tenant_id: PRD-mcphost-implicit-signup-mints-only-on-success R4.
--
-- `tenant_id` is the tenant an implicit signup created. `ON DELETE SET NULL`
-- keeps a rolled-back signup's row (it still counts toward the rate limiter)
-- with a NULL tenant.
ALTER TABLE signup_events ADD COLUMN tenant_id INTEGER REFERENCES tenants(id) ON DELETE SET NULL;
