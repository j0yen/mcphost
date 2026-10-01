-- compat: previous -- two wholly new tables; nothing existing changes shape.
-- mcphost 0058_lineage: PRD-mcphost-lineage-blast-radius requirement 3.
-- (Renumbered from this PRD's own 0057 during rebase: mcphost-chart-in-a-minute
-- claimed 0057 first, landing on main ahead of this branch.)
--
-- `lineage_nodes` is one row per dependency-graph node a tenant has ever
-- registered (`<kind>:<name>` is the node's id, e.g. `table:orders`,
-- `tool:my_tool`) -- `uses`/`last_used_unix` are bumped by the runs ledger
-- (requirement 5), `orphaned` is set by a confirmed `host.table.drop`
-- cascade (requirement 7) on a dependent chart/handle node.
--
-- `lineage_edges` is one row per upstream -> downstream dependency edge
-- (e.g. `table:orders -> tool:my_tool`, "this downstream node depends on
-- this upstream node") -- `evidence` records which registration path wrote
-- it (`source_scan`, `declared_reads`, `chain_publish`, `chart_store`,
-- `handle_materialize`, `document_put`, `schema`).
CREATE TABLE IF NOT EXISTS lineage_nodes (
    id             INTEGER PRIMARY KEY,
    tenant_id      INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    node_id        TEXT NOT NULL,
    kind           TEXT NOT NULL,
    label          TEXT NOT NULL,
    uses           INTEGER NOT NULL DEFAULT 0,
    created_unix   INTEGER NOT NULL,
    last_used_unix INTEGER,
    orphaned       INTEGER NOT NULL DEFAULT 0,
    UNIQUE(tenant_id, node_id)
);

CREATE TABLE IF NOT EXISTS lineage_edges (
    id            INTEGER PRIMARY KEY,
    tenant_id     INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    upstream_id   TEXT NOT NULL,
    downstream_id TEXT NOT NULL,
    evidence      TEXT NOT NULL,
    created_unix  INTEGER NOT NULL,
    UNIQUE(tenant_id, upstream_id, downstream_id)
);
CREATE INDEX IF NOT EXISTS idx_lineage_edges_tenant_upstream
    ON lineage_edges(tenant_id, upstream_id);
CREATE INDEX IF NOT EXISTS idx_lineage_edges_tenant_downstream
    ON lineage_edges(tenant_id, downstream_id);
