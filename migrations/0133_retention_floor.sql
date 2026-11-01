-- compat: previous -- one additive nullable column on `retention_policy`; an old release never selects it.
-- mcphost 0132_retention_floor: PRD-mcphost-synthetic-implicit-tenant-expiry.
--
-- `retention_policy.floor_unix` is when the tenant-expiry row was first
-- seeded; only tenants minted at or after it are ever expired, so the rows
-- that existed before the policy are counted (`would_expire`), never touched.
-- Set once by the first seed and never overwritten; NULL on every other row.
ALTER TABLE retention_policy ADD COLUMN floor_unix INTEGER;
