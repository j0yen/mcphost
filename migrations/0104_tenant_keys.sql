-- compat: previous -- one wholly new table plus one nullable column on
-- calls; an old release never queries either, no existing row's shape
-- changes.
-- mcphost 0104_tenant_keys: PRD-mcphost-key-scoped-to-tools R1.
--
-- A sub-key is a second hash-at-rest credential for a tenant, narrowed to an
-- allowlist of tools (`tools_json`, a JSON array of names). `tenants.key_hash`
-- is untouched by minting, using, or revoking one, so `host.key.rotate`
-- leaves every sub-key alive. `calls.key_id` is the sub-key's id for a call
-- authenticated by one, NULL for the primary key and every other credential.
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
