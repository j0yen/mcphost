-- compat: previous -- three wholly new tables an old release simply never
-- queries; no existing table's shape changes
-- (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0011_tenant_state: PRD-mcphost-tenant-state P0 requirement 1.
--
-- `tenant_state_kv` is the per-tenant key-value namespace
-- (`host.state.get/set/delete/list`): one row per (tenant, key), the value
-- kept as opaque `value_json` text (any JSON value serializes to a
-- string). `tenant_state_tables` is the schema registry a tenant's
-- `host.state.table_create` writes to; `tenant_state_rows` holds the rows
-- themselves, each row's fields as one `row_json` object rather than real
-- columns -- `tenant_state.rs` owns validating that object's shape against
-- the table's declared, typed schema before a row is ever written here, so
-- this migration doesn't need per-tenant DDL.
--
-- All three tables are new (unlike migration 0005's in-place rebuild),
-- so `ON DELETE CASCADE` on `tenant_id` needs no surgery -- it's just part
-- of the `CREATE TABLE`. Cascade-deleting a tenant already removes these
-- rows in the same transaction as every other child table.
CREATE TABLE IF NOT EXISTS tenant_state_kv (
    tenant_id    INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    key          TEXT NOT NULL,
    value_json   TEXT NOT NULL,
    updated_unix INTEGER NOT NULL,
    PRIMARY KEY (tenant_id, key)
);

CREATE TABLE IF NOT EXISTS tenant_state_tables (
    tenant_id    INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    table_name   TEXT NOT NULL,
    schema_json  TEXT NOT NULL,
    primary_key  TEXT,
    created_unix INTEGER NOT NULL,
    PRIMARY KEY (tenant_id, table_name)
);

CREATE TABLE IF NOT EXISTS tenant_state_rows (
    id         INTEGER PRIMARY KEY,
    tenant_id  INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    table_name TEXT NOT NULL,
    row_json   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_tenant_state_rows_tenant_table
    ON tenant_state_rows(tenant_id, table_name);
