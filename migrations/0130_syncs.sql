-- compat: previous -- two wholly new tables; an old release simply never
-- queries them (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0130_syncs: PRD-mcphost-sync-source-registry-and-create R2/R6.
--
-- `syncs` is one registered mirror of an external dataset into a host table;
-- `sync_runs` is the receipt a tick writes per run (schema fixed here so the
-- pull PRD only fills it). `running_since_unix` plus a 10-minute stale guard
-- covers a crashed tick.
CREATE TABLE IF NOT EXISTS syncs (
    id                 TEXT PRIMARY KEY,
    tenant_id          INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    kind               TEXT NOT NULL,
    ref                TEXT NOT NULL,
    secret             TEXT,
    table_name         TEXT NOT NULL,
    mode               TEXT NOT NULL,
    schedule           TEXT NOT NULL,
    primary_key        TEXT,
    status             TEXT NOT NULL,
    created_unix       INTEGER NOT NULL,
    next_run_unix      INTEGER,
    running_since_unix INTEGER
);
CREATE INDEX IF NOT EXISTS idx_syncs_tenant ON syncs(tenant_id);
CREATE INDEX IF NOT EXISTS idx_syncs_due ON syncs(next_run_unix);

CREATE TABLE IF NOT EXISTS sync_runs (
    id              TEXT PRIMARY KEY,
    sync_id         TEXT NOT NULL REFERENCES syncs(id) ON DELETE CASCADE,
    started_unix    INTEGER NOT NULL,
    duration_ms     INTEGER NOT NULL DEFAULT 0,
    status          TEXT NOT NULL,
    rows_added      INTEGER NOT NULL DEFAULT 0,
    rows_updated    INTEGER NOT NULL DEFAULT 0,
    rows_deleted    INTEGER NOT NULL DEFAULT 0,
    bytes           INTEGER NOT NULL DEFAULT 0,
    change_detected INTEGER NOT NULL DEFAULT 0,
    error           TEXT
);
CREATE INDEX IF NOT EXISTS idx_sync_runs_sync ON sync_runs(sync_id, started_unix);
