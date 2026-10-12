-- compat: previous -- one additive table (`dead_letters`) plus an index; an
-- old release never queries it, no existing statement's result set changes
-- (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0086_dead_letters: PRD-mcphost-event-trigger-from-webhook R1.
--
-- One row per refused `POST /hooks/...` delivery and per event-fired run
-- that finalized `error`. `headers_json` is already redacted to the
-- trigger's header allow list; `body` is the delivery text, cut to the
-- plan's `event_body_bytes_max` (`truncated` = 1 when it was cut).
CREATE TABLE IF NOT EXISTS dead_letters (
    id                TEXT PRIMARY KEY,
    tenant_id         INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    trigger_id        TEXT NOT NULL,
    received_unix     INTEGER NOT NULL,
    reason_code       TEXT NOT NULL,
    headers_json      TEXT NOT NULL DEFAULT '{}',
    body              TEXT NOT NULL DEFAULT '',
    truncated         INTEGER NOT NULL DEFAULT 0,
    run_id            TEXT,
    replayed_run_id   TEXT,
    error_class       TEXT
);

CREATE INDEX IF NOT EXISTS idx_dead_letters_tenant_trigger ON dead_letters(tenant_id, trigger_id, received_unix);
