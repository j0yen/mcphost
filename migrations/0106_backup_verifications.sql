-- compat: previous -- one wholly new table an old release never queries.
-- mcphost 0106_backup_verifications: PRD-mcphost-backup-restore-proof-nightly R3.
--
-- One row per `admin.backup_verified` call: the deploy tool's nightly job
-- digested the live file at backup time (`live_digest`), restored the
-- newest copy into a scratch dir and digested that (`restored_digest`);
-- `digest_match` is their equality, `age_s` is how old the copy was when
-- the verification was recorded, and `restored_counts` is the restored
-- file's `counts` object as JSON.
CREATE TABLE IF NOT EXISTS backup_verifications (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    verified_at     INTEGER NOT NULL,
    source          TEXT NOT NULL,
    taken_at_unix   INTEGER NOT NULL,
    age_s           INTEGER NOT NULL,
    live_digest     TEXT NOT NULL,
    restored_digest TEXT NOT NULL,
    digest_match    INTEGER NOT NULL,
    restored_counts TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX IF NOT EXISTS idx_backup_verifications_verified
    ON backup_verifications(verified_at);
