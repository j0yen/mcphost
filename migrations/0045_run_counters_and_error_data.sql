-- compat: previous -- additive columns; an old release simply never
-- queries them, and every existing run row's shape is unchanged
-- (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0045_run_counters_and_error_data: PRD-mcphost-run-result-overflow-to-state
-- requirements 2/3.
--
-- `counters_json` (requirement 3): the structured counters
-- (`items_processed`/`items_total`/`bytes_out`/`custom`) `host.progress`
-- writes onto a run, read back by `host.runs.get/wait/list` as `counters`.
-- NULL for every existing run (reads back as `{}`, per the PRD's own
-- Migration/compatibility section).
--
-- `error_data_json` (requirement 2): the structured `{needed_bytes,
-- available_bytes}`-style data alongside a run's existing `error_class`
-- (e.g. `state_quota`), surfaced as `error: {kind, data}`. NULL for every
-- existing run and every error path this PRD doesn't itself produce.
ALTER TABLE runs ADD COLUMN counters_json TEXT;
ALTER TABLE runs ADD COLUMN error_data_json TEXT;
