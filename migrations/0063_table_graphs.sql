-- compat: previous -- one wholly new table an old release simply never
-- queries; no existing table's shape changes
-- (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0063_table_graphs: PRD-mcphost-table-concept-graph requirement 2.
--
-- `table_graphs` holds the latest built `ConceptGraph` for one tenant --
-- one row per tenant (unlike `table_models`, which is one row per table),
-- since the graph is a single structure spanning every table the tenant's
-- `table_models` tick has computed a model for. `graph_json` is the whole
-- `tables_graph::ConceptGraph` (nodes + edges), serialized, so the shape can
-- grow without another migration -- same "opaque JSON blob, real columns
-- only for what's queried/filtered on" convention `table_models.model_json`
-- already uses. `stale` (0/1) is set the moment a write (`append`/`create`/
-- `drop`) or a `role`/`description` annotation (`host.table.model_set`)
-- changes a table this tenant's graph was built from, cleared once the
-- table-model tick's own rebuild (`tables_graph::tick_once`) recomputes it.
CREATE TABLE IF NOT EXISTS table_graphs (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    tenant_id    INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    version      INTEGER NOT NULL,
    graph_json   TEXT NOT NULL,
    computed_at  INTEGER NOT NULL,
    stale        INTEGER NOT NULL DEFAULT 0
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_table_graphs_tenant
    ON table_graphs(tenant_id);
CREATE INDEX IF NOT EXISTS idx_table_graphs_stale ON table_graphs(stale);
