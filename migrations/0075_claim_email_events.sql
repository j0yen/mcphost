-- compat: previous -- one wholly new table; an old release simply never
-- queries it, no existing row's shape changes.
-- mcphost 0075_claim_email_events: PRD-mcphost-ownership-moment requirement
-- 3 (AC3): a journal row for every claim-email send that ultimately failed
-- (both of `claim::send_with_retry`'s attempts) -- `status_code` is the
-- provider's own HTTP status (`NULL` for a pure transport failure), and
-- that is the ONLY thing about the failure this table ever stores: never
-- the provider's response body, never `MCPHOST_EMAIL_API_KEY`.
CREATE TABLE IF NOT EXISTS claim_email_events (
    id           INTEGER PRIMARY KEY,
    tenant_id    INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    status_code  INTEGER,
    created_unix INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_claim_email_events_tenant_created ON claim_email_events(tenant_id, created_unix);
