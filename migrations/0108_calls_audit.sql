-- compat: previous -- additive columns with defaults on an existing table; an
-- old release never queries them and every old row reads as `primary` / 0.
-- mcphost 0107_calls_audit: PRD-mcphost-call-audit-export R1/R2.
--
-- `calls.credential` names the credential kind that made the call (one enum,
-- `calllog::Credential`); `bytes_in` / `bytes_out` are the measured request
-- and response sizes (0 for rows written before this migration).
ALTER TABLE calls ADD COLUMN credential TEXT NOT NULL DEFAULT 'primary';
ALTER TABLE calls ADD COLUMN bytes_in INTEGER NOT NULL DEFAULT 0;
ALTER TABLE calls ADD COLUMN bytes_out INTEGER NOT NULL DEFAULT 0;
CREATE INDEX IF NOT EXISTS idx_calls_tenant_started ON calls(tenant_id, started_unix);
