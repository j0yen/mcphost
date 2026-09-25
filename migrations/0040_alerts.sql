-- compat: previous -- one wholly new table; an old release simply never
-- queries it, no existing row's shape changes, no existing statement's
-- result set changes (PRD-mcphost-migration-safety requirement 4, same
-- convention 0033/0034/0035's own compat notes already follow).
-- mcphost 0040_alerts: PRD-mcphost-alerting-webhook requirement 1.
--
-- `alerts`: one row per raised alert (or per collapsed-duplicate window --
-- see `repeat_count` below). `key` is the alert source's own stable name
-- (`signup.paused`, `quota.trip:<tenant>:<knob>`, `errors.rate`, ...);
-- `body_json` is the source's own structured detail, echoed verbatim into
-- the webhook POST and the inbox notice. `delivery_status` is one of
-- `pending` (row just inserted, delivery task not finished yet),
-- `delivered`, `failed`, `skipped` (stored, filtered by
-- `MCPHOST_ALERT_MIN_SEVERITY`), or `store-only` (neither sink configured).
CREATE TABLE IF NOT EXISTS alerts (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    key             TEXT NOT NULL,
    severity        TEXT NOT NULL,
    title           TEXT NOT NULL,
    body_json       TEXT NOT NULL,
    raised_at       INTEGER NOT NULL,
    delivered_at    INTEGER,
    delivery_status TEXT NOT NULL DEFAULT 'pending',
    acked_at        INTEGER,
    acked_by        TEXT,
    -- requirement 4: the cooldown collapse counter -- 0 on a fresh row,
    -- incremented once per duplicate raise collapsed into this row within
    -- `MCPHOST_ALERT_COOLDOWN_SECS` of its own `raised_at`.
    repeat_count    INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_alerts_key ON alerts(key);
CREATE INDEX IF NOT EXISTS idx_alerts_key_raised_at ON alerts(key, raised_at);
CREATE INDEX IF NOT EXISTS idx_alerts_acked_at ON alerts(acked_at);
