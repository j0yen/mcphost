-- compat: previous -- one wholly new table an old release never queries; no
-- existing table's shape changes.
-- mcphost 0113_example_proofs: PRD-mcphost-example-proof-nightly-and-hide R2.
--
-- One row per example name: the latest nightly proof verdict. A failing
-- proof sets hidden_reason (and red_since on the first red of a streak);
-- a passing one clears both. Hidden state lives here, not in the binary.
CREATE TABLE IF NOT EXISTS example_proofs (
    name          TEXT PRIMARY KEY,
    ok            INTEGER NOT NULL,
    at            INTEGER NOT NULL,
    duration_s    REAL NOT NULL DEFAULT 0,
    log_excerpt   TEXT NOT NULL DEFAULT '',
    runner        TEXT NOT NULL DEFAULT '',
    hidden_reason TEXT,
    red_since     INTEGER
);
