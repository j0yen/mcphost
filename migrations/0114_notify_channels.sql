-- compat: previous -- three wholly new tables; an old release simply never
-- queries them (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0113_notify_channels: PRD-mcphost-notify-user-channel.
--
-- `notify_channels` is one place a tenant's human can be reached: `kind` is
-- a `notify::ChannelKind` name, `target_enc`/`target_nonce` the encrypted
-- address or URL (a Slack incoming-webhook URL is a bearer), `target_hint`
-- the part that may be shown back. `consecutive_failures` reaching the cap
-- sets `disabled_unix`.
-- `notifies` is one `host.notify.send`; `notify_receipts` is one row per
-- (notify, channel). `channel_id` carries no foreign key so removing a
-- channel keeps its receipts.
CREATE TABLE IF NOT EXISTS notify_channels (
    id                   TEXT PRIMARY KEY,
    tenant_id            INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    kind                 TEXT NOT NULL,
    target_enc           BLOB NOT NULL,
    target_nonce         BLOB NOT NULL,
    target_hint          TEXT NOT NULL,
    created_unix         INTEGER NOT NULL,
    consecutive_failures INTEGER NOT NULL DEFAULT 0,
    disabled_unix        INTEGER
);
CREATE INDEX IF NOT EXISTS idx_notify_channels_tenant ON notify_channels(tenant_id, created_unix);
CREATE TABLE IF NOT EXISTS notifies (
    id           TEXT PRIMARY KEY,
    tenant_id    INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    text         TEXT NOT NULL,
    level        TEXT NOT NULL,
    link         TEXT,
    created_unix INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_notifies_tenant ON notifies(tenant_id, created_unix);
CREATE TABLE IF NOT EXISTS notify_receipts (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    notify_id       TEXT NOT NULL REFERENCES notifies(id) ON DELETE CASCADE,
    tenant_id       INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    channel_id      TEXT NOT NULL,
    kind            TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT 'queued',
    upstream_status INTEGER,
    attempts        INTEGER NOT NULL DEFAULT 0,
    created_unix    INTEGER NOT NULL,
    delivered_unix  INTEGER,
    UNIQUE (notify_id, channel_id)
);
CREATE INDEX IF NOT EXISTS idx_notify_receipts_tenant ON notify_receipts(tenant_id, created_unix);
