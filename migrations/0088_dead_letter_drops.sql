-- compat: previous -- one additive table (`dead_letter_drops`); an old
-- release never queries it, no existing statement's result set changes
-- (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0087_dead_letter_drops: PRD-mcphost-event-trigger-from-webhook R3/R4.
--
-- `dropped`: how many of a tenant's dead letters the `dead_letters_max`
-- ring has evicted so far, so `host.trigger.dead_letters` can say how much
-- it no longer holds.
CREATE TABLE IF NOT EXISTS dead_letter_drops (
    tenant_id INTEGER PRIMARY KEY REFERENCES tenants(id) ON DELETE CASCADE,
    dropped   INTEGER NOT NULL DEFAULT 0
);
