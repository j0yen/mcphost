-- compat: previous -- one wholly new table plus an index; an old release
-- simply never queries it, no existing row's shape changes.
-- mcphost 0095_draft_events: PRD-mcphost-first-tool-from-description R5.
--
-- One row per `host.tool_from_description` call that was admitted under the
-- tenant's `drafts_per_day` and the host-wide `MCPHOST_DRAFT_DAILY_CAP`. The
-- row is inserted (ok = 0) before the model is asked, so concurrent calls
-- count each other; it is updated with the outcome and token counts after.
-- Only token counts are stored, never the prompt or source.
CREATE TABLE IF NOT EXISTS draft_events (
    id                INTEGER PRIMARY KEY,
    tenant_id         INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    ts                INTEGER NOT NULL,
    ok                INTEGER NOT NULL DEFAULT 0,
    attempts          INTEGER NOT NULL DEFAULT 0,
    prompt_tokens     INTEGER NOT NULL DEFAULT 0,
    completion_tokens INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_draft_events_tenant_ts ON draft_events(tenant_id, ts);
CREATE INDEX IF NOT EXISTS idx_draft_events_ts ON draft_events(ts);
