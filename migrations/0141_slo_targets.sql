-- compat: previous -- one wholly new table and one new index; no existing column or table is altered,
-- and an old release ignores both (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0141_slo_targets: PRD-mcphost-slo-targets-on-status R4/R7.
--
-- `slo_daily`: one row per `(day, key)`, `day` an RFC 3339 UTC date (`"YYYY-MM-DD"`, sorting
-- lexicographically like `status_daily.day`), `key` one of the `SLO_TARGETS` keys. Written by the
-- daily status rollup so a 30-day view survives `calls` pruning; `measured` is NULL when the day had
-- fewer than the minimum samples, `sample_n` says how many it had. Never pruned.
CREATE TABLE IF NOT EXISTS slo_daily (
    day       TEXT NOT NULL,
    key       TEXT NOT NULL,
    measured  REAL,
    sample_n  INTEGER NOT NULL,
    PRIMARY KEY (day, key)
);

-- The 24 h `slo` numbers read `calls` by `started_unix` alone; every existing `calls` index leads
-- with `tenant_id`, so without this the window is a full scan. The trailing columns are the ones the
-- window query reads (`ok`, `duration_ms`, `error_class`, plus `origin`/`outcome`, which SQLite
-- counts as read because the statement names them), so the scan is answered from the index without
-- a row lookup per call. The `WHERE` is the window query's own `origin`/`outcome` filter, so probe
-- and test rows are never in the index and the query evaluates no per-row filter.
-- `Db::slo_calls_window` spells the identical `WHERE` terms; change one, change both.
CREATE INDEX IF NOT EXISTS idx_calls_started_unix
    ON calls(started_unix, ok, duration_ms, error_class, origin, outcome)
    WHERE origin <> 'probe' AND outcome <> 'test';
