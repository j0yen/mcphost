-- compat: previous -- one wholly new table an old release simply never
-- queries; no existing table's shape changes (PRD-mcphost-migration-safety
-- requirement 4).
-- mcphost 0015_triggers: PRD-mcphost-schedules P0 requirement 1.
--
-- `triggers` is the first (and, per the PRD's goal 3, deliberately generic)
-- member of the trigger family: a schedule today, an inbound event later,
-- both the same row shape (`kind` distinguishes them, `config_json` carries
-- whatever that kind needs -- a cron expression and optional args/tz for
-- `'schedule'`). The scheduler tick (`triggers::spawn_scheduler`) selects
-- `kind = 'schedule'` rows with `next_unix <= now`, enqueues a run through
-- the same `runs` ledger migration 0014 added (`trigger = 'schedule'`,
-- `trigger_ref = triggers.id`), and advances `next_unix` to the following
-- occurrence.
--
-- `config_hash` (sha256 hex of `config_json`) backs the requirement-1
-- uniqueness constraint without hashing a value SQLite would otherwise have
-- to compare byte-for-byte on every insert -- the same "hash for a UNIQUE
-- index" shape `tenants.key_hash` already uses for keys.
--
-- New table, like migration 0011's `tenant_state_*` and 0014's `runs` --
-- `ON DELETE CASCADE` on `tenant_id` needs no surgery, just part of the
-- `CREATE TABLE` (requirement 1: "Cascade on tenant delete").
CREATE TABLE IF NOT EXISTS triggers (
    id               TEXT PRIMARY KEY,
    tenant_id        INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    tool_name        TEXT NOT NULL,
    kind             TEXT NOT NULL,
    config_json      TEXT NOT NULL,
    config_hash      TEXT NOT NULL,
    enabled          INTEGER NOT NULL DEFAULT 1,
    created_unix     INTEGER NOT NULL,
    next_unix        INTEGER,
    last_run_id      TEXT,
    last_fired_unix  INTEGER,
    UNIQUE (tenant_id, tool_name, kind, config_hash)
);

CREATE INDEX IF NOT EXISTS idx_triggers_tenant_tool ON triggers(tenant_id, tool_name);
CREATE INDEX IF NOT EXISTS idx_triggers_due ON triggers(kind, enabled, next_unix);
