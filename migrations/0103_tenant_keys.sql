-- compat: previous -- additive table + one nullable column; an old release never reads either.
-- mcphost 0103_tenant_keys: PRD-mcphost-key-scoped-to-tools R1.
--
-- A sub-key authenticates like the tenant key but carries a tool allowlist.
-- Stored hash-at-rest exactly like `tenants.key_hash`; `tenants` is untouched.
CREATE TABLE IF NOT EXISTS tenant_keys (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    tenant_id    INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    key_hash     TEXT NOT NULL UNIQUE,
    label        TEXT NOT NULL,
    tools_json   TEXT NOT NULL,
    created_at   INTEGER NOT NULL,
    expires_at   INTEGER NOT NULL,
    revoked_at   INTEGER,
    last_used_at INTEGER,
    UNIQUE (tenant_id, label)
);
ALTER TABLE calls ADD COLUMN key_id INTEGER;
