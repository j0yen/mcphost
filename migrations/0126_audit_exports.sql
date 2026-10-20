-- compat: previous -- one wholly new table; an old release simply never
-- queries it (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0126_audit_exports: PRD-mcphost-audit-export-tamper-evident R3.
--
-- One row per `host.audit.export`: the signed root and the window it covered,
-- so `host.audit.verify {root_hash}` can recompute the same chain later. The
-- export itself stays a pure function of the three audit tables; nothing is
-- added to them. `since`/`until` are NULL for an open end.
CREATE TABLE IF NOT EXISTS audit_exports (
    tenant_id    INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    root_hash    TEXT NOT NULL,
    since        INTEGER,
    until        INTEGER,
    rows         INTEGER NOT NULL,
    created_unix INTEGER NOT NULL,
    PRIMARY KEY (tenant_id, root_hash)
);
