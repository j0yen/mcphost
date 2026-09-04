-- compat: previous -- baseline schema; nothing precedes it to be compatible with.
-- mcphost 0001_init: tenants, tools, secrets, calls, logs, signup rate-limit ledger.

CREATE TABLE IF NOT EXISTS tenants (
    id           INTEGER PRIMARY KEY,
    namespace    TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    key_hash     TEXT NOT NULL UNIQUE,
    created_at   TEXT NOT NULL,
    disabled     INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS tools (
    id          INTEGER PRIMARY KEY,
    tenant_id   INTEGER NOT NULL REFERENCES tenants(id),
    name        TEXT NOT NULL,
    kind        TEXT NOT NULL,
    spec        TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    UNIQUE(tenant_id, name)
);

CREATE TABLE IF NOT EXISTS secrets (
    id          INTEGER PRIMARY KEY,
    tenant_id   INTEGER NOT NULL REFERENCES tenants(id),
    name        TEXT NOT NULL,
    value_enc   BLOB NOT NULL,
    nonce       BLOB NOT NULL,
    created_at  TEXT NOT NULL,
    UNIQUE(tenant_id, name)
);

CREATE TABLE IF NOT EXISTS calls (
    id           INTEGER PRIMARY KEY,
    tenant_id    INTEGER NOT NULL REFERENCES tenants(id),
    tool_name    TEXT NOT NULL,
    started_at   TEXT NOT NULL,
    started_unix INTEGER NOT NULL,
    duration_ms  INTEGER NOT NULL,
    ok           INTEGER NOT NULL,
    error_class  TEXT
);

CREATE INDEX IF NOT EXISTS idx_calls_tenant_started ON calls(tenant_id, started_unix);
CREATE INDEX IF NOT EXISTS idx_calls_tenant_tool_started ON calls(tenant_id, tool_name, started_unix);

CREATE TABLE IF NOT EXISTS logs (
    id         INTEGER PRIMARY KEY,
    tenant_id  INTEGER NOT NULL REFERENCES tenants(id),
    tool_name  TEXT NOT NULL,
    ts         TEXT NOT NULL,
    line       TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_logs_tenant_tool ON logs(tenant_id, tool_name, id);

CREATE TABLE IF NOT EXISTS signup_events (
    id         INTEGER PRIMARY KEY,
    source_ip  TEXT NOT NULL,
    created_unix INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_signup_events_ip_time ON signup_events(source_ip, created_unix);
