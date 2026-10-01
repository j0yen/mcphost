-- compat: previous -- one wholly new table; an old release simply never
-- queries it (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0057_chart_index: PRD-mcphost-chart-in-a-minute P1 requirement 6.
--
-- A chart's rows live in the sharing tenant's own per-tenant table file
-- (`_mcphost_charts`, see `chart.rs`'s module doc), keyed by chart id --
-- but `GET /charts/{id}` has no tenant in its own URL, so this small global
-- table is the id -> tenant_id index that lets that unauthenticated route
-- find which tenant's file to open before verifying the request's
-- signature. Holds only the id/tenant_id/created_unix a lookup needs --
-- never the spec or caption themselves, which stay in the per-tenant file
-- alongside the rest of that tenant's own table store.
CREATE TABLE IF NOT EXISTS chart_index (
    id           TEXT PRIMARY KEY,
    tenant_id    INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    created_unix INTEGER NOT NULL
);
