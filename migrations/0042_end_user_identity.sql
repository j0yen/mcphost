-- compat: previous -- additive columns/tables; an old release simply never
-- queries the new ones, and every existing row's shape is either unchanged
-- (calls, tenant_state_rows: ADD COLUMN with a default) or carried forward
-- unchanged into a rebuilt table (tenant_state_kv: see below)
-- (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0042_end_user_identity: PRD-mcphost-end-user-identity requirement 4.
--
-- `calls` gains the end user a call ran as, when any: `end_user_subject`/
-- `end_user_issuer` (issuer only ever set for `method = 'oauth'`) and
-- `end_user_method` (`'oauth'` or `'assertion'`). Indexed alongside
-- `tenant_id` for `host.runs.list`-style per-tenant-per-end-user lookups.
ALTER TABLE calls ADD COLUMN end_user_subject TEXT;
ALTER TABLE calls ADD COLUMN end_user_issuer TEXT;
ALTER TABLE calls ADD COLUMN end_user_method TEXT;

CREATE INDEX IF NOT EXISTS idx_calls_tenant_end_user ON calls(tenant_id, end_user_subject);

-- requirement 5: `tenant_state_kv`'s primary key grows `end_user_subject`
-- so u1 and u2 can each write key "k" without colliding -- SQLite's
-- PRIMARY KEY can't be altered in place, so this is a rebuild (same
-- "CREATE ... _new, INSERT ... SELECT, DROP, RENAME" shape migration
-- 0005_cascade_delete.sql already uses), not an `ALTER TABLE ADD COLUMN`.
-- `end_user_subject` is `NOT NULL DEFAULT ''` -- SQLite's PRIMARY KEY
-- uniqueness treats two NULLs as distinct (never colliding), which would
-- silently defeat the "one tenant-wide row per key" invariant every
-- existing row (backfilled to `''` below) depends on; `''` is never a
-- valid end-user subject (every real one comes from a JWT `sub` or a
-- signed assertion's `sub`, both non-empty), so it's a safe sentinel for
-- "tenant-wide, no end user".
CREATE TABLE tenant_state_kv_new (
    tenant_id        INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    key              TEXT NOT NULL,
    end_user_subject TEXT NOT NULL DEFAULT '',
    value_json       TEXT NOT NULL,
    updated_unix     INTEGER NOT NULL,
    PRIMARY KEY (tenant_id, key, end_user_subject)
);
INSERT INTO tenant_state_kv_new (tenant_id, key, end_user_subject, value_json, updated_unix)
    SELECT tenant_id, key, '', value_json, updated_unix FROM tenant_state_kv;
DROP TABLE tenant_state_kv;
ALTER TABLE tenant_state_kv_new RENAME TO tenant_state_kv;

-- requirement 5: `tenant_state_rows` scoping -- no primary-key rebuild
-- needed here (rows are addressed by the synthetic `id`, never by a
-- natural key), so a plain `ADD COLUMN` with the same `''` sentinel
-- suffices. Composite index for `host.state.query {end_user: "self"}`'s
-- pushed-down `WHERE end_user_subject = ?` (AC6).
ALTER TABLE tenant_state_rows ADD COLUMN end_user_subject TEXT NOT NULL DEFAULT '';
CREATE INDEX IF NOT EXISTS idx_tenant_state_rows_tenant_table_enduser
    ON tenant_state_rows(tenant_id, table_name, end_user_subject);

-- P1 requirement 6 (AC9): which distinct end-user subjects wrote state for
-- a tenant, and when they last did -- `end_users_max`'s "distinct end-user
-- subjects that wrote state in the trailing 30 days" is a `COUNT(*)` over
-- this table `WHERE last_write_unix >= ?`, not a scan of every KV/row
-- write.
CREATE TABLE IF NOT EXISTS tenant_end_user_activity (
    tenant_id        INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    end_user_subject TEXT NOT NULL,
    last_write_unix  INTEGER NOT NULL,
    PRIMARY KEY (tenant_id, end_user_subject)
);
CREATE INDEX IF NOT EXISTS idx_tenant_end_user_activity_tenant_last_write
    ON tenant_end_user_activity(tenant_id, last_write_unix);
