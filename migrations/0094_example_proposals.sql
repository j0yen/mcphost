-- compat: previous -- one wholly new table plus indexes; an old release
-- simply never queries it, no existing row's shape changes.
-- mcphost 0094_example_proposals: PRD-mcphost-example-from-published-tool R4.
--
-- One row per `host.example.propose`. `rendered_tar` is the in-memory tar of
-- the rendered `examples/<name>/` directory (<= 256 KB). `status` is one of
-- pending, approved, rejected, merged; `pr_url` is set when the pull command
-- opened the PR. Deleting a tenant cascades its proposals.
CREATE TABLE IF NOT EXISTS example_proposals (
    id            INTEGER PRIMARY KEY,
    tenant_id     INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    tool          TEXT NOT NULL,
    name          TEXT NOT NULL,
    version       INTEGER NOT NULL,
    status        TEXT NOT NULL DEFAULT 'pending',
    rendered_tar  BLOB NOT NULL,
    created_at    INTEGER NOT NULL,
    decided_at    INTEGER,
    decided_by    TEXT,
    reason        TEXT,
    pr_url        TEXT
);
CREATE INDEX IF NOT EXISTS idx_example_proposals_tenant ON example_proposals(tenant_id, status);
CREATE INDEX IF NOT EXISTS idx_example_proposals_status ON example_proposals(status, created_at);
