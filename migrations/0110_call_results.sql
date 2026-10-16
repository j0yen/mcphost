-- compat: previous -- one wholly new table an old release never queries; no
-- existing table's shape changes.
-- mcphost 0110_call_results: PRD-mcphost-idempotency-key-on-call R2.
--
-- One row per (tenant, tool, idempotency key). A row with both result_json
-- and error_json NULL is a call still running (its claim); finishing the call
-- fills exactly one of them. Rows expire 24 h after the first call and are
-- purged by the nightly prune.
CREATE TABLE IF NOT EXISTS call_results (
    tenant_id    INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    tool_name    TEXT    NOT NULL,
    key          TEXT    NOT NULL,
    args_sha256  TEXT    NOT NULL,
    result_json  TEXT,
    error_json   TEXT,
    created_unix INTEGER NOT NULL,
    expires_unix INTEGER NOT NULL,
    UNIQUE (tenant_id, tool_name, key)
);
CREATE INDEX IF NOT EXISTS idx_call_results_expires ON call_results(expires_unix);
