-- compat: previous -- one wholly new table plus an index; an old release
-- simply never queries it, no existing row's shape changes.
-- mcphost 0100_trigger_receipts: PRD-mcphost-scheduled-job-receipts R1.
--
-- One row per finished run a trigger fired (source schedule, event or
-- manual). `output_excerpt` is at most 240 characters plus an ellipsis,
-- redacted like `host.tool_logs`. Streak state is derived from these rows,
-- never kept as a counter, so a purge cannot desynchronize it.
CREATE TABLE IF NOT EXISTS trigger_receipts (
    id             INTEGER PRIMARY KEY,
    tenant_id      INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    trigger_id     TEXT NOT NULL,
    tool_name      TEXT NOT NULL,
    run_id         TEXT NOT NULL,
    source         TEXT NOT NULL,
    started_unix   INTEGER NOT NULL,
    duration_ms    INTEGER NOT NULL,
    status         TEXT NOT NULL,
    error_code     TEXT,
    output_excerpt TEXT NOT NULL DEFAULT '',
    cost_calls     INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX IF NOT EXISTS idx_trigger_receipts_trigger ON trigger_receipts (tenant_id, trigger_id, id);
CREATE INDEX IF NOT EXISTS idx_trigger_receipts_started ON trigger_receipts (started_unix);
