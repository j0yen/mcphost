-- compat: previous -- one wholly new table plus one index; an old release
-- never queries it, no existing row's shape changes.
-- mcphost 0105_email_inbound: PRD-mcphost-inbound-email-address-per-tenant R7.
--
-- One row per accepted delivery to `POST /email/inbound` (a refused
-- signature writes nothing). `tenant_id` is NULL for a mail addressed to no
-- known tenant; `outcome` is `accepted` or `unknown_recipient`.
CREATE TABLE IF NOT EXISTS email_inbound_log (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    tenant_id           INTEGER REFERENCES tenants(id) ON DELETE CASCADE,
    provider_message_id TEXT NOT NULL,
    outcome             TEXT NOT NULL,
    received_at         TEXT NOT NULL,
    bytes               INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_email_inbound_log_tenant ON email_inbound_log(tenant_id, id);
