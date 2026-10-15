-- compat: previous -- one wholly new table plus two indexes; an old release
-- never queries it, no existing row's shape changes.
-- mcphost 0102_tasks: PRD-mcphost-task-handoff-tool R2.
--
-- A task is a thread with a state machine the host owns. `thread_id` is
-- filled in right after the carrying message commits (NULL for that instant
-- only); deleting either tenant deletes the row, and deleting the thread
-- deletes it too.
CREATE TABLE IF NOT EXISTS tasks (
    id                  TEXT PRIMARY KEY,
    thread_id           TEXT REFERENCES threads(id) ON DELETE CASCADE,
    poster_tenant_id    INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    recipient_tenant_id INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    status              TEXT NOT NULL,
    payload_json        TEXT NOT NULL,
    result_json         TEXT,
    created_unix        INTEGER NOT NULL,
    deadline_unix       INTEGER NOT NULL,
    claimed_unix        INTEGER,
    done_unix           INTEGER,
    dead_unix           INTEGER,
    dead_reason         TEXT,
    dedupe_key          TEXT
);
CREATE INDEX IF NOT EXISTS idx_tasks_poster_status ON tasks (poster_tenant_id, status);
CREATE INDEX IF NOT EXISTS idx_tasks_status_deadline ON tasks (status, deadline_unix);
