-- compat: previous -- one defaulted column on `orgs`; an old release never reads it.
-- mcphost 0127_org_digest: PRD-mcphost-org-cost-attribution R3.
--
-- digest_enabled: 1 sends the owner the weekly usage email, 0 opts out
-- (`host.org.digest.set`). Default 1, so every existing org is opted in.
ALTER TABLE orgs ADD COLUMN digest_enabled INTEGER NOT NULL DEFAULT 1;
