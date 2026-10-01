-- compat: previous -- one additive `agent_profiles.hints` column (ALTER
-- TABLE, defaulting every existing row to enabled) plus two wholly new
-- tables; nothing existing changes shape.
-- mcphost 0060_next_hint: PRD-mcphost-one-next-tool requirement 4 (AC4,
-- AC5, AC7), requirement 5 (AC6), requirement 7 (AC9).
--
-- `host_tool_usage` is the per-tenant set of host.* control-plane tool
-- names that have ever succeeded for that tenant -- the "distinct tools
-- used" count requirement 4's five-tool cutoff reads, kept separate from
-- the pre-existing `calls` table (which only ever logs a tenant's OWN
-- published-tool invocations, via `host.tool_call`/a direct namespaced
-- call, not a control-plane verb like `host.tool_publish`/`host.state.set`
-- itself) rather than widening that table's population and risking every
-- existing `host.usage` `calls`/`errors`/`p50_ms`/`p95_ms` fixture.
--
-- `hint_events` is one row per `next` hint actually shown -- `followed` is
-- set when the tenant's very next successful `host.*` call named the same
-- tool (requirement 7 / AC9's `host.usage` rollup); `resolved` marks that
-- the next-call check has already happened for this row, so a later,
-- unrelated call can never retroactively flip an old hint to followed.
--
-- `agent_profiles.hints` (requirement 5 / AC6): `host.agent.profile_set
-- (hints = false)` flips this to 0, which suppresses `next` from every
-- later `host.*` result for the tenant; defaults to enabled (1) for every
-- tenant, including rows that predate this column.
CREATE TABLE IF NOT EXISTS host_tool_usage (
    tenant_id       INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    tool_name       TEXT NOT NULL,
    first_used_unix INTEGER NOT NULL,
    PRIMARY KEY (tenant_id, tool_name)
);

CREATE TABLE IF NOT EXISTS hint_events (
    id          INTEGER PRIMARY KEY,
    tenant_id   INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    hinted_tool TEXT NOT NULL,
    shown_unix  INTEGER NOT NULL,
    resolved    INTEGER NOT NULL DEFAULT 0,
    followed    INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_hint_events_tenant_shown ON hint_events(tenant_id, shown_unix);
CREATE INDEX IF NOT EXISTS idx_hint_events_tenant_resolved ON hint_events(tenant_id, resolved);

ALTER TABLE agent_profiles ADD COLUMN hints INTEGER NOT NULL DEFAULT 1;
