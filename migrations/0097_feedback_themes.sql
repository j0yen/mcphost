-- compat: previous -- one wholly new table plus one additive nullable column
-- on `feedback`; an old release never queries either
-- (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0097_feedback_themes: PRD-mcphost-feedback-themes-nightly R1.
--
-- One row per theme: `theme_id` is `th_` + 12 hex of
-- sha256(kind|error_code|tool|text_prefix). `tenants_json` (distinct, <= 20),
-- `example_ids_json` (feedback ids, <= 5, oldest first) and `call_ids_json`
-- (union of members' last_call_ids, <= 25) are JSON arrays. `error_code` and
-- `tool` are '' when a row carried none.
CREATE TABLE IF NOT EXISTS feedback_themes (
    theme_id         TEXT PRIMARY KEY,
    kind             TEXT NOT NULL,
    error_code       TEXT NOT NULL DEFAULT '',
    tool             TEXT NOT NULL DEFAULT '',
    text_prefix      TEXT NOT NULL DEFAULT '',
    count            INTEGER NOT NULL DEFAULT 0,
    tenants_json     TEXT NOT NULL DEFAULT '[]',
    example_ids_json TEXT NOT NULL DEFAULT '[]',
    call_ids_json    TEXT NOT NULL DEFAULT '[]',
    first_seen_unix  INTEGER NOT NULL,
    last_seen_unix   INTEGER NOT NULL,
    status           TEXT NOT NULL DEFAULT 'open'
);
CREATE INDEX IF NOT EXISTS idx_feedback_themes_last ON feedback_themes(last_seen_unix);
ALTER TABLE feedback ADD COLUMN theme_id TEXT;
CREATE INDEX IF NOT EXISTS idx_feedback_theme ON feedback(theme_id);
