-- compat: previous -- two wholly new tables an old release never queries;
-- no existing table or column is touched, one `tenant_clients` row is
-- backfilled per existing tenant.
-- mcphost 0117_tenant_clients: PRD-mcphost-same-user-second-agent R1/R3.
--
-- `tenant_clients`: one row per client (agent connection) of a tenant. The
-- tenant's pre-existing credentials (its key, its `/u/` secret) are client
-- 0, id `c0-<tenant id>`, label `first`, `secret_hash` NULL (client 0's
-- secret stays on `tenants`); a sibling that joined through a join code has
-- its own `secret_hash` (a `/u/<secret>/mcp` URL secret, hash-at-rest like
-- `tenants.url_secret_hash`). `retired_secret_hash` is the secret this
-- client held before its last rotation, kept so the old URL answers
-- `tenant_key_invalid` instead of an anonymous 404. `revoked_unix` set by
-- `host.client.revoke`.
CREATE TABLE IF NOT EXISTS tenant_clients (
    id                  TEXT PRIMARY KEY,
    tenant_id           INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    secret_hash         TEXT UNIQUE,
    retired_secret_hash TEXT,
    label               TEXT NOT NULL,
    client_name         TEXT,
    client_version      TEXT,
    created_unix        INTEGER NOT NULL,
    last_seen_unix      INTEGER,
    revoked_unix        INTEGER
);

CREATE INDEX IF NOT EXISTS idx_tenant_clients_tenant ON tenant_clients(tenant_id);
CREATE INDEX IF NOT EXISTS idx_tenant_clients_retired ON tenant_clients(retired_secret_hash);

-- `join_codes`: the tenant's join code (`/j/<code>/mcp`). `code_hash` is the
-- SHA-256 hex of the plaintext; `code_plain` is kept so `host.whoami` can
-- print `join_url` on every read (the same convention as
-- `invites.code_plain`). One live code per tenant (rotation revokes the old
-- row and inserts a new one).
CREATE TABLE IF NOT EXISTS join_codes (
    id           INTEGER PRIMARY KEY,
    code_hash    TEXT NOT NULL UNIQUE,
    code_plain   TEXT NOT NULL,
    tenant_id    INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    max_uses     INTEGER NOT NULL DEFAULT 5,
    uses         INTEGER NOT NULL DEFAULT 0,
    expires_unix INTEGER NOT NULL,
    revoked_unix INTEGER,
    created_unix INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_join_codes_tenant ON join_codes(tenant_id);

-- `consent_cookies`: the OAuth consent page's "remember this browser" cookie
-- (R6). The cookie value is a random token; only its SHA-256 hex is stored,
-- naming the tenant the browser consented to. Expired rows are ignored on
-- read.
CREATE TABLE IF NOT EXISTS consent_cookies (
    token_hash   TEXT PRIMARY KEY,
    tenant_id    INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    created_unix INTEGER NOT NULL,
    expires_unix INTEGER NOT NULL
);

INSERT OR IGNORE INTO tenant_clients (id, tenant_id, secret_hash, label, created_unix)
    SELECT 'c0-' || id, id, NULL, 'first', COALESCE(created_unix, 0) FROM tenants;
