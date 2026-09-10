-- compat: previous -- one wholly new table an old release simply never
-- queries; no existing table's shape changes (PRD-mcphost-migration-safety
-- requirement 4).
-- mcphost 0014_runs: PRD-mcphost-runs-and-jobs P0 requirement 1.
--
-- `runs` is the one ledger every execution writes to, whatever triggered
-- it: an ordinary synchronous `tools/call` (`trigger = 'call'`, written by
-- `Db::record_call_attributed` in the same transaction as its `calls` row),
-- an async job (`trigger = 'job'`, `host.tool_call(..., async=true)`), or
-- (not yet built -- the next three PRDs) a schedule/event/chain step.
-- `id` is a ulid (see `state::new_ulid`): lexicographically sortable by
-- creation time, so the executor's leasing query (`ORDER BY id`) needs no
-- second column for FIFO order within a tenant.
--
-- New table, like migration 0011's `tenant_state_*` -- `ON DELETE CASCADE`
-- on `tenant_id` needs no surgery, just part of the `CREATE TABLE`.
-- Two columns are additive beyond the PRD's literal requirement-1 list:
-- `purged_unix` (`host.runs.purge` needs to distinguish "a job that never
-- produced a result" from "a done job whose result was purged" when both
-- read back as `result_ref IS NULL`, AC7) and `args_json` (a `queued` run's
-- call arguments have to survive from `host.tool_call(..., async=true)`'s
-- insert to the executor's later lease -- the two happen in different
-- transactions, potentially seconds apart, so the arguments need a column
-- of their own; nothing in requirement 1's column list carries them
-- otherwise). Both are the same backward-compatible-additive-column shape
-- migration 0009's `outcome` used.
CREATE TABLE IF NOT EXISTS runs (
    id               TEXT PRIMARY KEY,
    tenant_id        INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    tool_name        TEXT NOT NULL,
    trigger          TEXT NOT NULL,
    trigger_ref      TEXT,
    caller_tenant_id INTEGER,
    status           TEXT NOT NULL,
    progress_json    TEXT,
    result_ref       TEXT,
    error_class      TEXT,
    started_unix     INTEGER,
    finished_unix    INTEGER,
    duration_ms      INTEGER,
    deadline_s       INTEGER,
    attempt          INTEGER NOT NULL DEFAULT 1,
    purged_unix      INTEGER,
    args_json        TEXT
);

CREATE INDEX IF NOT EXISTS idx_runs_tenant_started ON runs(tenant_id, started_unix);
CREATE INDEX IF NOT EXISTS idx_runs_tenant_status ON runs(tenant_id, status, id);
CREATE INDEX IF NOT EXISTS idx_runs_tenant_tool ON runs(tenant_id, tool_name, id);
CREATE INDEX IF NOT EXISTS idx_runs_status ON runs(status, id);
