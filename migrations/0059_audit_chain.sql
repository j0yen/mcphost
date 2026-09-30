-- compat: previous -- one wholly new table; an old release simply never
-- queries it (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0059_audit_chain: PRD-mcphost-row-policy requirement 6.
--
-- `audit_chain` is the hash-chained record of every filtered read
-- (`plane = 'sql' | 'docs'`), one chain per tenant (technical
-- considerations: "Chain per tenant. Genesis hash constant per tenant
-- chain."), ordered by `id`. `record_hash` commits to every other column
-- plus `prior_record_hash` (`src/rowpolicy/audit.rs::compute_record_hash`);
-- `prior_record_hash` is the previous record's `record_hash` for this
-- tenant, or the fixed genesis constant for the first. `withheld_count` is
-- nullable: requirement 6 computes it only under the 10,000-candidate
-- bound, null otherwise.
CREATE TABLE IF NOT EXISTS audit_chain (
    id                 INTEGER PRIMARY KEY,
    tenant_id          INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    subject            TEXT NOT NULL,
    plane              TEXT NOT NULL,
    policy_hash        TEXT NOT NULL,
    applied            TEXT NOT NULL,
    returned_count     INTEGER NOT NULL,
    withheld_count     INTEGER,
    timestamp_unix     INTEGER NOT NULL,
    request_id         TEXT NOT NULL,
    prior_record_hash  TEXT NOT NULL,
    record_hash        TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_audit_chain_tenant_id ON audit_chain(tenant_id, id DESC);
CREATE INDEX IF NOT EXISTS idx_audit_chain_tenant_subject ON audit_chain(tenant_id, subject, id DESC);
