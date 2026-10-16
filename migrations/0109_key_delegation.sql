-- compat: previous -- four additive columns with defaults plus one index on
-- `tenant_keys`; an old release never selects them, and every existing row
-- (a `host.key.mint` sub-key) reads as a depth-1 `["mcp"]` key with no
-- parent that may not delegate, which is exactly how it behaved.
-- mcphost 0109_key_delegation: PRD-mcphost-key-delegate-child-keys R1.
--
-- `parent_key_id` NULL = a child of the primary key (depth 0, scopes
-- `["mcp"]`, no row of its own). `revoked_by` names the label a cascade (or
-- a plain revoke) was aimed at, so a descendant's `key_revoked` says who.
ALTER TABLE tenant_keys ADD COLUMN parent_key_id INTEGER REFERENCES tenant_keys(id);
ALTER TABLE tenant_keys ADD COLUMN scopes_json TEXT NOT NULL DEFAULT '["mcp"]';
ALTER TABLE tenant_keys ADD COLUMN can_delegate INTEGER NOT NULL DEFAULT 0;
ALTER TABLE tenant_keys ADD COLUMN depth INTEGER NOT NULL DEFAULT 1;
ALTER TABLE tenant_keys ADD COLUMN revoked_by TEXT;
CREATE INDEX IF NOT EXISTS idx_tenant_keys_parent ON tenant_keys (tenant_id, parent_key_id);
