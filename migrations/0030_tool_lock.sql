-- compat: previous -- one wholly new table (`tool_lock`; CREATE TABLE IF
-- NOT EXISTS, same precedent as migrations 0011/0014/0015/0017/0019/0020/
-- 0021/0024/0026). An old release never queries this table, so nothing
-- about its existing behavior changes (PRD-mcphost-migration-safety
-- requirement 4).
--
-- mcphost 0030_tool_lock: PRD-mcphost-python-dependency-policy requirement 1.
--
-- `tool_lock(tenant_id, name, version, lock_text, resolved_unix,
-- advisories_json, audited_unix)` is the durable record of what
-- `host.tool_publish` resolved a python tool's `requirements` to
-- (`uv pip compile --generate-hashes`, or a caller-supplied pre-hashed lock
-- -- requirement 5) -- one row per published version, mirroring
-- `tool_versions`' own per-version shape. `advisories_json`/`audited_unix`
-- are the *only* columns a later daily re-audit (requirement 6) ever
-- updates in place, without a republish: `lock_text`/`resolved_unix` are
-- fixed at publish time, matching every version's spec being immutable.
CREATE TABLE IF NOT EXISTS tool_lock (
    tenant_id       INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    version         INTEGER NOT NULL,
    lock_text       TEXT NOT NULL,
    resolved_unix   INTEGER NOT NULL,
    advisories_json TEXT NOT NULL DEFAULT '[]',
    audited_unix    INTEGER,
    PRIMARY KEY (tenant_id, name, version)
);
CREATE INDEX IF NOT EXISTS idx_tool_lock_tenant_name ON tool_lock(tenant_id, name);
