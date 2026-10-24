-- compat: previous -- two additive `NOT NULL DEFAULT 0` columns on `calls`, one
-- index, one tiny new table; an old release never selects the columns and
-- ignores the index and the table (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0136_calls_progress: PRD-mcphost-tools-call-progress-notifications P0.
--
-- `progress_token` is 1 when the `tools/call` carried `_meta.progressToken`;
-- `progress_frames` is how many `notifications/progress` frames the host sent
-- for it. Both are written once, with the row, at call end. The index serves
-- the 7-day admin-healthz aggregate over every tenant (every existing `calls`
-- index leads with a tenant column). `progress_epoch` holds the unix time of
-- this migration so a read before day 7 is visibly partial (`progress_7d.since`).
ALTER TABLE calls ADD COLUMN progress_token INTEGER NOT NULL DEFAULT 0;
ALTER TABLE calls ADD COLUMN progress_frames INTEGER NOT NULL DEFAULT 0;
CREATE INDEX IF NOT EXISTS idx_calls_started ON calls(started_unix);
CREATE TABLE IF NOT EXISTS progress_epoch (
    since_unix INTEGER NOT NULL
);
INSERT INTO progress_epoch (since_unix)
    SELECT CAST(strftime('%s', 'now') AS INTEGER)
    WHERE NOT EXISTS (SELECT 1 FROM progress_epoch);
