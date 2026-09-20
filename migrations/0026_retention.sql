-- compat: previous -- two wholly new tables (`retention_policy`,
-- `prune_log`; CREATE TABLE IF NOT EXISTS, same precedent as migrations
-- 0011/0014/0015/0017/0019/0020/0021/0024) plus a one-time `auto_vacuum`
-- pragma switch + `VACUUM` rebuild (done in Rust, see
-- `Db::migrate_0026_retention`, not in this batch, since it needs to run
-- conditionally and `VACUUM` cannot run inside `execute_batch`'s implicit
-- transaction). An old release simply never queries either new table and
-- never reads `PRAGMA auto_vacuum`, so nothing about its existing behavior
-- changes (PRD-mcphost-migration-safety requirement 4).
--
-- mcphost 0026_retention: PRD-mcphost-data-retention P0 requirements 1-3.
--
-- `retention_policy(table_name, days, updated_unix)` is the per-table
-- window every prune cycle reads (`Db::retention_windows`), seeded (and
-- re-synced on every `serve` start via `Db::seed_retention_policy_from_env`,
-- so an operator's env change takes effect on restart) from
-- `$MCPHOST_RETENTION_<TABLE>_DAYS` -- see `retention::POLICY_TABLES` for
-- the full table list and its defaults.
--
-- `prune_log(id, started_unix, finished_unix, ok, error, deleted_json)` is
-- the nightly prune's own durable history -- `/healthz`'s `last_prune_ok`
-- and `admin.usage`'s `last_prune` both read only the newest row
-- (`Db::usage_size_stats`).
CREATE TABLE IF NOT EXISTS retention_policy (
    table_name   TEXT PRIMARY KEY,
    days         INTEGER NOT NULL,
    updated_unix INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS prune_log (
    id            INTEGER PRIMARY KEY,
    started_unix  INTEGER NOT NULL,
    finished_unix INTEGER,
    ok            INTEGER NOT NULL DEFAULT 0,
    error         TEXT,
    deleted_json  TEXT NOT NULL DEFAULT '{}'
);
