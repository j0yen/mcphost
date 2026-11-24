-- compat: previous -- one nullable column on `orgs`; an old release never reads it and every
-- existing row carries forward with NULL (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0142_org_offboard: PRD-mcphost-org-export-and-offboard-v2 R4/R5.
--
-- `orgs.offboarded_unix`: when the owner ran `host.org.offboard`; NULL while the org is switched on.
-- `host.org.restore` clears it. The members' own `tenants.disabled`/`disabled_reason = 'org_offboard'`
-- rows are what actually refuse their keys; this column answers "is this org offboarded" without
-- scanning them. The migration wrapper (`Db::migrate_0142_org_offboard`) skips the batch once the
-- column exists, because ALTER is not idempotent.
ALTER TABLE orgs ADD COLUMN offboarded_unix INTEGER;
