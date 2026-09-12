-- compat: previous -- one wholly new table an old release simply never
-- queries; no existing table's shape changes (PRD-mcphost-migration-safety
-- requirement 4).
-- mcphost 0017_event_dedupe: PRD-mcphost-inbound-events P1 requirement 7 /
-- AC11.
--
-- `host.trigger.set(kind="event", ..., dedupe_header="X-GitHub-Delivery")`
-- names a header whose value a sender never repeats except on its own
-- retry; `event_dedupe` remembers the first run a given
-- `(trigger_id, dedupe_key)` pair produced, so `POST /hooks/...`'s second
-- delivery of the same id answers 202 with the *same* `run_id` instead of
-- running the tool twice. `PRIMARY KEY (trigger_id, dedupe_key)` is the
-- whole mechanism -- a second insert attempt for a seen pair is a plain
-- `UNIQUE` violation the caller (`hooks::claim_dedupe`) reads back as
-- "already claimed" rather than a distinct lookup-then-insert race.
--
-- `ON DELETE CASCADE` on `trigger_id`: removing a trigger frees its dedupe
-- history too, same cascade shape migration 0015's `triggers.tenant_id`
-- already uses.
CREATE TABLE IF NOT EXISTS event_dedupe (
    trigger_id   TEXT NOT NULL REFERENCES triggers(id) ON DELETE CASCADE,
    dedupe_key   TEXT NOT NULL,
    run_id       TEXT NOT NULL,
    created_unix INTEGER NOT NULL,
    PRIMARY KEY (trigger_id, dedupe_key)
);

CREATE INDEX IF NOT EXISTS idx_event_dedupe_created ON event_dedupe(created_unix);
