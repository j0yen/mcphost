-- compat: previous -- one additive column on `sync_runs` and one wholly new
-- table; an old release never selects either.
-- mcphost 0131_sync_pull: PRD-mcphost-sync-pull-with-change-detection R1/R3/R4.
--
-- `sync_runs.provider_calls` is how many provider requests the run made (the
-- per-tick budget counts them). `sync_cursors` holds what the next pull sends
-- back to the provider (`etag`, `modified`) and compares against (`body_hash`),
-- the row count of the last snapshot (`last_rows`, for `drift`), and when the
-- one `auth_expired` inbox notice went out (`notified_unix`, cleared by the
-- next ok run).
ALTER TABLE sync_runs ADD COLUMN provider_calls INTEGER NOT NULL DEFAULT 0;
CREATE TABLE IF NOT EXISTS sync_cursors (
    sync_id       TEXT PRIMARY KEY REFERENCES syncs(id) ON DELETE CASCADE,
    etag          TEXT,
    modified      TEXT,
    body_hash     TEXT,
    last_rows     INTEGER NOT NULL DEFAULT 0,
    notified_unix INTEGER
);
