-- compat: previous -- additive: `tools.current_version` (default 1, so
-- every existing row keeps reading as "version 1 current") and a wholly new
-- `tool_versions`/`tool_version_watermarks` pair; every existing read of
-- `tools.spec`/`tools.kind` is unaffected (PRD-mcphost-migration-safety
-- requirement 4) -- an unpinned `host.tool_call` still reads those two
-- columns exactly as before, now kept in sync with `tool_versions`'s own
-- current row by `Db::upsert_tool`/`Db::rollback_tool_version`.
-- mcphost 0028_tool_versions: PRD-mcphost-tool-versions requirement 1.
--
-- Migration/compatibility: every existing tool becomes version 1
-- (backfilled in Rust, since `source_sha256` needs a real SHA-256 over each
-- row's own spec text -- see `Db::migrate_0028_tool_versions`),
-- `current_version = 1`.
ALTER TABLE tools ADD COLUMN current_version INTEGER NOT NULL DEFAULT 1;

CREATE TABLE IF NOT EXISTS tool_versions (
    id            INTEGER PRIMARY KEY,
    tenant_id     INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    name          TEXT NOT NULL,
    version       INTEGER NOT NULL,
    kind          TEXT NOT NULL,
    spec          TEXT NOT NULL,
    created_at    TEXT NOT NULL,
    created_unix  INTEGER NOT NULL,
    source_sha256 TEXT NOT NULL,
    UNIQUE(tenant_id, name, version)
);
CREATE INDEX IF NOT EXISTS idx_tool_versions_tenant_name ON tool_versions(tenant_id, name, version);

-- PRD requirement 6 (AC7): per (owner tool, caller) watermark of the last
-- version that caller's unpinned call actually saw -- what `version_changed`
-- is computed off (once per real change, never repeated on a following call
-- that saw no further change).
CREATE TABLE IF NOT EXISTS tool_version_watermarks (
    tenant_id        INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    name             TEXT NOT NULL,
    caller_tenant_id INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    last_version     INTEGER NOT NULL,
    PRIMARY KEY (tenant_id, name, caller_tenant_id)
);
