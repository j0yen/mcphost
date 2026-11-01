-- compat: previous -- one wholly new table and one additive nullable column on `tenants`; an old release never selects either.
-- mcphost 0134_tenant_lifecycle_events: PRD-mcphost-external-unclaimed-tenant-expiry.
--
-- `tenant_lifecycle_events` journals the stages of the external unclaimed
-- policy (`warned`, `warn_no_address`, `warn_failed`, `invalidated`,
-- `notice_failed`, `restored`, `purged`); a stage counts for one idle run
-- only when `created_unix` is newer than that run's `idle_since_unix`.
-- `tenants.claim_bound_email` is the one address a warn-issued claim token
-- accepts; NULL for every token issued any other way.
CREATE TABLE IF NOT EXISTS tenant_lifecycle_events (
    id              INTEGER PRIMARY KEY,
    tenant_id       INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    policy          TEXT NOT NULL,
    stage           TEXT NOT NULL,
    idle_since_unix INTEGER,
    status_code     INTEGER,
    created_unix    INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_tenant_lifecycle_events_tenant_created ON tenant_lifecycle_events(tenant_id, created_unix);

ALTER TABLE tenants ADD COLUMN claim_bound_email TEXT;
