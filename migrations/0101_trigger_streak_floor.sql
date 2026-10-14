-- compat: previous -- one wholly new table; an old release never queries it.
-- mcphost 0101_trigger_streak_floor: PRD-mcphost-scheduled-job-receipts R3.
--
-- `host.trigger.resume` records the newest receipt id a trigger had at that
-- moment; the failure streak counts only receipts after it, so a resumed
-- schedule starts again at 0 while the receipt history stays intact.
CREATE TABLE IF NOT EXISTS trigger_streak_floor (
    trigger_id TEXT PRIMARY KEY,
    floor_id   INTEGER NOT NULL
);
