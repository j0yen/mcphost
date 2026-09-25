-- compat: previous -- two wholly new tables an old release simply never
-- queries; no existing table's shape changes
-- (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0039_table_models: PRD-mcphost-table-semantic-model requirement 1.
--
-- `table_models` holds the latest generated semantic model for one
-- (tenant, table) pair -- one row per declared `host.table.*` table, the
-- whole model serialized as `model_json` so the inference shape can grow
-- without another migration. `stale` (0/1) is requirement 5's signal: set
-- the moment `append`/`create` change the table's rows, cleared once the
-- background tick recomputes. `table_model_annotations` holds requirement
-- 6's agent overrides (`role`, `unit`, `description`, `hidden`), one row
-- per (tenant, table, column, key) -- `column_name` is `''` for a
-- table-level annotation (no per-column key clash: `is_valid_ident`
-- already forbids an empty declared column name in `host.table.create`).
CREATE TABLE IF NOT EXISTS table_models (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    tenant_id    INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    table_name   TEXT NOT NULL,
    version      INTEGER NOT NULL,
    model_json   TEXT NOT NULL,
    row_count    INTEGER NOT NULL,
    computed_at  INTEGER NOT NULL,
    stale        INTEGER NOT NULL DEFAULT 0
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_table_models_tenant_table
    ON table_models(tenant_id, table_name);
CREATE INDEX IF NOT EXISTS idx_table_models_stale ON table_models(stale);

CREATE TABLE IF NOT EXISTS table_model_annotations (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    tenant_id    INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    table_name   TEXT NOT NULL,
    column_name  TEXT NOT NULL DEFAULT '',
    key          TEXT NOT NULL,
    value        TEXT NOT NULL,
    updated_at   INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_table_model_annotations_unique
    ON table_model_annotations(tenant_id, table_name, column_name, key);
