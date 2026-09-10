-- compat: previous -- additive columns/tables with safe defaults; every
-- existing tool becomes 'private' automatically and no existing call path
-- for a private tool changes (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0013_sharing: PRD-mcphost-sharing P0 requirements 1-6.
--
-- `tools.visibility` ('private'|'group'|'public', default 'private') is
-- the resolution switch the cross-tenant arm of `handler.rs`'s `call_tool`
-- reads. `share_description` is the catalog-facing blurb an owner sets at
-- share time (NULL until shared). `shared_unix` is when this tool was last
-- (re)shared, NULL while private. `shared_group` names the group a
-- `visibility = 'group'` tool is shared to (NULL for 'private'/'public').
-- `unshared_by` records who most recently unshared a tool ('admin' when
-- `admin.tool_unshare` did it, NULL otherwise -- including a tool that was
-- never shared, or one the owner unshared itself) so `host.tool_list` can
-- surface AC9's `unshared_by: admin` without a second table.
ALTER TABLE tools ADD COLUMN visibility TEXT NOT NULL DEFAULT 'private';
ALTER TABLE tools ADD COLUMN share_description TEXT;
ALTER TABLE tools ADD COLUMN shared_unix INTEGER;
ALTER TABLE tools ADD COLUMN shared_group TEXT;
ALTER TABLE tools ADD COLUMN unshared_by TEXT;

CREATE INDEX IF NOT EXISTS idx_tools_visibility ON tools(visibility);

-- A tenant's own named allow-lists for `visibility = 'group'` tools. A
-- group is owned by exactly one tenant (the sharer); members are other
-- tenants by id. `ON DELETE CASCADE` on both, consistent with migration
-- 0005's cascade-delete-on-tenant-removal contract.
CREATE TABLE IF NOT EXISTS groups (
    id              INTEGER PRIMARY KEY,
    owner_tenant_id INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    UNIQUE(owner_tenant_id, name)
);

CREATE TABLE IF NOT EXISTS group_members (
    id               INTEGER PRIMARY KEY,
    group_id         INTEGER NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    member_tenant_id INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    created_at       TEXT NOT NULL,
    UNIQUE(group_id, member_tenant_id)
);

-- `calls.caller_tenant_id` is who actually placed a call when it crosses
-- tenants (NULL for every same-tenant call, which is unaffected) -- this is
-- the attribution AC1 asks for ("the run and call rows carry
-- caller_tenant_id"); there is no separate `runs` table in this schema,
-- `calls` is the one per-invocation ledger, so this column alone carries
-- that attribution. `tenant_id` on the row stays the tool's OWNER (whose
-- sandbox and secrets ran it) so every existing per-tool/per-owner usage
-- query is unaffected; `caller_tenant_id` is purely additive.
ALTER TABLE calls ADD COLUMN caller_tenant_id INTEGER REFERENCES tenants(id);
CREATE INDEX IF NOT EXISTS idx_calls_caller_started ON calls(caller_tenant_id, started_unix);
