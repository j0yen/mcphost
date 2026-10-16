-- compat: previous -- four additive nullable columns plus one index on
-- `tenant_keys`; an old release never selects them, and every existing row
-- reads as an unbound key, which is exactly how it behaved.
-- mcphost 0111_key_binding: PRD-mcphost-delegation-bound-to-run-or-session R1.
--
-- `bound_kind` 'run' ties a key to one `runs.id`, 'session' to one MCP
-- connection's `Mcp-Session-Id`; `bound_ref` is that id. `bound_last_seen_unix`
-- is the session idle clock (written at most once a minute). `revoked_reason`
-- is why a bound key died: run_<state>, session_idle, session_closed.
ALTER TABLE tenant_keys ADD COLUMN bound_kind TEXT CHECK (bound_kind IN ('run', 'session'));
ALTER TABLE tenant_keys ADD COLUMN bound_ref TEXT;
ALTER TABLE tenant_keys ADD COLUMN bound_last_seen_unix INTEGER;
ALTER TABLE tenant_keys ADD COLUMN revoked_reason TEXT;
CREATE INDEX IF NOT EXISTS idx_tenant_keys_bound ON tenant_keys (tenant_id, bound_kind, bound_ref);
