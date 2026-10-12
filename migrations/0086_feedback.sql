-- compat: previous -- one wholly new table plus indexes; an old release
-- simply never queries it, no existing row's shape changes.
-- mcphost 0083_feedback: PRD-mcphost-feedback-tool-and-ledger requirement 2.
--
-- One row per `host.feedback.send`. `tenant_id` is NULL for an anonymous
-- sender; deleting a tenant cascades its rows. `last_call_ids` is a JSON
-- array (oldest first, at most 5). `ip_hash` is the one-way hash of the
-- sender's source address and is never returned by any verb or the CLI.
CREATE TABLE IF NOT EXISTS feedback (
    id             INTEGER PRIMARY KEY,
    feedback_id    TEXT NOT NULL UNIQUE,
    tenant_id      INTEGER REFERENCES tenants(id) ON DELETE CASCADE,
    kind           TEXT NOT NULL,
    text           TEXT NOT NULL,
    context_json   TEXT,
    client_name    TEXT,
    client_version TEXT,
    last_call_ids  TEXT NOT NULL DEFAULT '[]',
    ip_hash        TEXT NOT NULL,
    created_unix   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_feedback_created ON feedback(created_unix);
CREATE INDEX IF NOT EXISTS idx_feedback_tenant ON feedback(tenant_id, created_unix);
CREATE INDEX IF NOT EXISTS idx_feedback_ip ON feedback(ip_hash, created_unix);
