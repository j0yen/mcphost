-- compat: previous -- one wholly new table; an old release simply never
-- queries it (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0083_call_traces: PRD-mcphost-call-trace-replay requirement 2.
--
-- One bounded trace per tenant tool call, keyed by the `calls.id` the
-- call's result returns as `call_id`. `request_json` is the sanitized
-- request (secrets masked), `stdout_tail`/`stderr_tail` the last bytes the
-- tool wrote, `timings_json` the phase timings, `error_json` the parsed
-- failure (`code`, `message`, `line`, `source_line`) of a failed call.
-- ON DELETE CASCADE ties a trace to its `calls` row (and so to
-- `admin.tenant_delete`). Retention (newest 200 per tenant, 24 h) is
-- enforced by the housekeeping tick, never by this schema.
CREATE TABLE IF NOT EXISTS call_traces (
    call_id       INTEGER PRIMARY KEY REFERENCES calls(id) ON DELETE CASCADE,
    tenant_id     INTEGER NOT NULL,
    request_json  TEXT NOT NULL,
    stdout_tail   TEXT NOT NULL DEFAULT '',
    stderr_tail   TEXT NOT NULL DEFAULT '',
    timings_json  TEXT NOT NULL DEFAULT '{}',
    error_json    TEXT,
    created_unix  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_call_traces_tenant_created
    ON call_traces (tenant_id, created_unix);
