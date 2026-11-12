-- compat: previous -- two wholly new tables and their indexes; no existing table or column is altered, and an
-- old release never queries them (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0141_queues: PRD-mcphost-queue-primitive-v2 R1.
--
-- `queues` is one tenant's named batch container; `queue_items` is one pushed item, which the dispatcher
-- turns into an ordinary `runs` row with `trigger = 'queue'` (`runs` gains no column). `state` holds a
-- `QueueState::as_str()` spelling (src/events.rs); `status` is the item's own four-value lifecycle.
-- `idx` is the item's position in push order within its queue (identity, never reused); `run_id` names the
-- `runs` row an item was started as; `attempts` is 1 once started (retries are a later PRD).
CREATE TABLE IF NOT EXISTS queues (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    tenant_id     INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    name          TEXT    NOT NULL,
    consumer_tool TEXT    NOT NULL,
    concurrency   INTEGER NOT NULL,
    state         TEXT    NOT NULL,
    created_at_ms INTEGER NOT NULL,
    drained_at_ms INTEGER,
    UNIQUE (tenant_id, name)
);

CREATE TABLE IF NOT EXISTS queue_items (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    queue_id     INTEGER NOT NULL REFERENCES queues(id) ON DELETE CASCADE,
    idx          INTEGER NOT NULL,
    item_json    TEXT    NOT NULL,
    status       TEXT    NOT NULL CHECK (status IN ('pending', 'running', 'done', 'failed')),
    run_id       TEXT,
    attempts     INTEGER NOT NULL DEFAULT 0,
    last_error   TEXT,
    pushed_at_ms INTEGER NOT NULL,
    ended_at_ms  INTEGER
);

CREATE INDEX IF NOT EXISTS idx_queue_items_queue_status_idx ON queue_items(queue_id, status, idx);
CREATE INDEX IF NOT EXISTS idx_queue_items_run_id ON queue_items(run_id);
