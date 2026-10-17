-- compat: previous -- one wholly new table an old release never queries; no
-- existing table's shape changes.
-- mcphost 0117_perf_samples: PRD-mcphost-performance-guarantees-measured R3.
--
-- One row per (hourly cycle, plan, path): p50/p95 over `n` probe calls.
CREATE TABLE IF NOT EXISTS perf_samples (
    id     INTEGER PRIMARY KEY,
    ts     INTEGER NOT NULL,
    plan   TEXT    NOT NULL,
    path   TEXT    NOT NULL,
    p50_ms INTEGER NOT NULL,
    p95_ms INTEGER NOT NULL,
    n      INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_perf_samples_plan_path_ts ON perf_samples(plan, path, ts);
