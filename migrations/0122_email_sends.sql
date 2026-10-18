-- compat: previous -- a wholly new table; an old release never queries it.
-- mcphost 0122_email_sends: PRD-mcphost-send-email-from-tool R6.
--
-- One row per `host.email.send` attempt that passed argument validation.
-- `status` is an `email_send::SendStatus` name; `provider_message_id` is what
-- the provider answered for a `sent` row; `error` the provider status for a
-- `provider_error` row. `tool` is the calling tool run's name ('' for the
-- agent itself). `tenant_id` carries no cascade-sensitive history, so the
-- rows go with the tenant.
CREATE TABLE IF NOT EXISTS email_sends (
    id                  TEXT PRIMARY KEY,
    tenant_id           INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    tool                TEXT NOT NULL DEFAULT '',
    from_addr           TEXT NOT NULL,
    to_addrs            TEXT NOT NULL,
    subject             TEXT NOT NULL,
    status              TEXT NOT NULL,
    provider_message_id TEXT,
    error               TEXT,
    reply_to_message_id TEXT,
    sent_unix           INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_email_sends_tenant ON email_sends(tenant_id, sent_unix);
