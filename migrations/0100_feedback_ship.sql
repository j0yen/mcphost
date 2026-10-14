-- compat: previous -- three additive nullable columns on `feedback_themes` and
-- one wholly new table; an old release never queries any of them
-- (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0100_feedback_ship: PRD-mcphost-feedback-closed-loop-notice R1/R3.
--
-- `shipped_version` / `shipped_anchor` / `shipped_unix` are set together when a
-- theme ships (`admin.feedback.ship` or the nightly CHANGELOG match); `status`
-- becomes 'shipped'. `feedback_notices` records one row per (theme, tenant)
-- so a second ship never sends a second inbox message.
ALTER TABLE feedback_themes ADD COLUMN shipped_version TEXT;
ALTER TABLE feedback_themes ADD COLUMN shipped_anchor TEXT;
ALTER TABLE feedback_themes ADD COLUMN shipped_unix INTEGER;
CREATE TABLE IF NOT EXISTS feedback_notices (
    theme_id    TEXT NOT NULL,
    tenant_id   INTEGER NOT NULL,
    message_id  TEXT NOT NULL,
    sent_unix   INTEGER NOT NULL,
    PRIMARY KEY (theme_id, tenant_id)
);
