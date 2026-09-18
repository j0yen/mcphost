-- compat: previous -- `tools.spec` is a tool's single live version; a
-- republish (`Db::upsert_tool`) overwrote it in place and the prior source
-- was gone (PRD-mcphost-tool-versions problem statement). This migration
-- is additive only: `tools` keeps every existing column and every existing
-- row's shape, gaining one new column (`current_version`, defaulting every
-- existing row to 1); `tool_versions` and `shared_tool_last_seen` are new
-- tables, touching nothing that already exists.
--
-- mcphost 0026_tool_versions: PRD-mcphost-tool-versions P0 requirement 1.
--
-- Every `host.tool_publish` becomes an immutable, numbered row here
-- instead of an in-place overwrite; `tools.current_version` is the
-- pointer `host.tool_rollback` moves. `source_sha256` is the sha256 of
-- this version's serialized `spec` (the same JSON blob `tools.spec`
-- already stores the current one of) rather than a `source`-field-only
-- hash: a spec-less-of-source kind (`echo`'s spec is a JSON Schema, not
-- code; `http`'s is a request template) still gets a meaningful, distinct
-- hash per version this way, so `host.tool_history`'s "distinct
-- source_sha256" (AC1) holds for every kind, not just `python`.
CREATE TABLE IF NOT EXISTS tool_versions (
    id            INTEGER PRIMARY KEY,
    tenant_id     INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    name          TEXT NOT NULL,
    version       INTEGER NOT NULL,
    kind          TEXT NOT NULL,
    spec          TEXT NOT NULL,
    created_unix  INTEGER NOT NULL,
    source_sha256 TEXT NOT NULL,
    UNIQUE(tenant_id, name, version)
);

CREATE INDEX IF NOT EXISTS idx_tool_versions_tenant_name
    ON tool_versions(tenant_id, name, version);

-- P1 requirement 6 (AC7): the last version a cross-tenant caller of a
-- shared tool has actually seen -- `NULL`/absent means "never called it
-- unpinned before", which is why a caller's very first unpinned call never
-- carries a `version_changed` note (nothing to diff against yet).
CREATE TABLE IF NOT EXISTS shared_tool_last_seen (
    owner_tenant_id  INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    name             TEXT NOT NULL,
    caller_tenant_id INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    version          INTEGER NOT NULL,
    PRIMARY KEY (owner_tenant_id, name, caller_tenant_id)
);

-- Migration / compatibility: "Existing tools become version 1 on
-- migration; current_version = 1" -- this column's default handles the
-- pointer half; the row backfill (a version-1 `tool_versions` row for
-- every tool published before this migration) happens in Rust
-- (`Db::migrate_0026_tool_versions`), since computing `source_sha256`
-- needs a hash function no bare SQL statement here has access to.
ALTER TABLE tools ADD COLUMN current_version INTEGER NOT NULL DEFAULT 1;
