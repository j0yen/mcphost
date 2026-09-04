-- mcphost 0005_cascade_delete: PRD-mcphost-tenant-delete requirements 3/4.
--
-- SQLite cannot ALTER a foreign key in place, so each child table that
-- references tenants(id) is recreated with ON DELETE CASCADE and its rows
-- copied across, in the order logs, calls, secrets, tools,
-- registry_documents -- inside one BEGIN IMMEDIATE so a crash mid-migration
-- leaves the previous (uncascaded) schema intact rather than a half-migrated
-- one. foreign_keys is off for the duration (SQLite refuses to toggle it
-- inside a transaction anyway, and nothing here needs FK enforcement
-- mid-surgery) and back on once the swap has committed.
--
-- Also adds admin_events (requirement 4: one audit row per delete,
-- deliberately its own table so `admin.usage`, which only reads `calls`,
-- is unaffected by it -- AC8).
--
-- `signup_events` is keyed by source IP, not tenant (PRD technical
-- considerations); it carries no tenant_id column, so it is left alone.
PRAGMA foreign_keys=OFF;

BEGIN IMMEDIATE;

CREATE TABLE logs_new (
    id         INTEGER PRIMARY KEY,
    tenant_id  INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    tool_name  TEXT NOT NULL,
    ts         TEXT NOT NULL,
    line       TEXT NOT NULL
);
INSERT INTO logs_new (id, tenant_id, tool_name, ts, line)
    SELECT id, tenant_id, tool_name, ts, line FROM logs;
DROP TABLE logs;
ALTER TABLE logs_new RENAME TO logs;
CREATE INDEX IF NOT EXISTS idx_logs_tenant_tool ON logs(tenant_id, tool_name, id);

CREATE TABLE calls_new (
    id           INTEGER PRIMARY KEY,
    tenant_id    INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    tool_name    TEXT NOT NULL,
    started_at   TEXT NOT NULL,
    started_unix INTEGER NOT NULL,
    duration_ms  INTEGER NOT NULL,
    ok           INTEGER NOT NULL,
    error_class  TEXT,
    cpu_ms       INTEGER,
    peak_rss_kb  INTEGER
);
INSERT INTO calls_new (id, tenant_id, tool_name, started_at, started_unix, duration_ms, ok, error_class, cpu_ms, peak_rss_kb)
    SELECT id, tenant_id, tool_name, started_at, started_unix, duration_ms, ok, error_class, cpu_ms, peak_rss_kb FROM calls;
DROP TABLE calls;
ALTER TABLE calls_new RENAME TO calls;
CREATE INDEX IF NOT EXISTS idx_calls_tenant_started ON calls(tenant_id, started_unix);
CREATE INDEX IF NOT EXISTS idx_calls_tenant_tool_started ON calls(tenant_id, tool_name, started_unix);

CREATE TABLE secrets_new (
    id          INTEGER PRIMARY KEY,
    tenant_id   INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    value_enc   BLOB NOT NULL,
    nonce       BLOB NOT NULL,
    created_at  TEXT NOT NULL,
    UNIQUE(tenant_id, name)
);
INSERT INTO secrets_new (id, tenant_id, name, value_enc, nonce, created_at)
    SELECT id, tenant_id, name, value_enc, nonce, created_at FROM secrets;
DROP TABLE secrets;
ALTER TABLE secrets_new RENAME TO secrets;

CREATE TABLE tools_new (
    id          INTEGER PRIMARY KEY,
    tenant_id   INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    kind        TEXT NOT NULL,
    spec        TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    UNIQUE(tenant_id, name)
);
INSERT INTO tools_new (id, tenant_id, name, kind, spec, created_at)
    SELECT id, tenant_id, name, kind, spec, created_at FROM tools;
DROP TABLE tools;
ALTER TABLE tools_new RENAME TO tools;

CREATE TABLE registry_documents_new (
    tenant_id    INTEGER PRIMARY KEY REFERENCES tenants(id) ON DELETE CASCADE,
    namespace    TEXT NOT NULL,
    document     TEXT NOT NULL,
    published_at TEXT NOT NULL
);
INSERT INTO registry_documents_new (tenant_id, namespace, document, published_at)
    SELECT tenant_id, namespace, document, published_at FROM registry_documents;
DROP TABLE registry_documents;
ALTER TABLE registry_documents_new RENAME TO registry_documents;
CREATE INDEX IF NOT EXISTS idx_registry_documents_namespace ON registry_documents(namespace);

CREATE TABLE IF NOT EXISTS admin_events (
    id     INTEGER PRIMARY KEY,
    ts     TEXT NOT NULL,
    action TEXT NOT NULL,
    tenant TEXT,
    detail TEXT NOT NULL
);

COMMIT;

PRAGMA foreign_keys=ON;
