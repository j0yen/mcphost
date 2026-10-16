-- compat: previous -- one wholly new table an old release never queries; no
-- existing table's shape changes.
-- mcphost 0113_restarts: PRD-mcphost-graceful-drain-contract R2.
--
-- One row per graceful shutdown: what was in flight at the signal, how many
-- finished inside the drain bound and how many were interrupted.
CREATE TABLE IF NOT EXISTS restarts (
    restart_id          TEXT    PRIMARY KEY,
    signal_at_unix      INTEGER NOT NULL,
    in_flight_at_signal INTEGER NOT NULL,
    finished            INTEGER NOT NULL,
    dropped             INTEGER NOT NULL,
    drain_ms            INTEGER NOT NULL,
    bound_s             INTEGER NOT NULL,
    version             TEXT    NOT NULL
);
