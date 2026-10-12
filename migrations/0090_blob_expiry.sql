-- compat: previous -- two additive columns on `blobs` (nullable `expires_at`,
-- defaulted `missing`); an old release never selects them and its
-- `INSERT`s name their columns, so no statement's result set changes
-- (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0086_blob_expiry: PRD-mcphost-blob-retention-and-quota R1/R3.
--
-- `expires_at` (unix seconds, NULL = never) is the blob's expiry; `missing`
-- is set by the nightly prune when a row's file is gone from disk.
ALTER TABLE blobs ADD COLUMN expires_at INTEGER;
ALTER TABLE blobs ADD COLUMN missing INTEGER NOT NULL DEFAULT 0;
CREATE INDEX IF NOT EXISTS idx_blobs_expires ON blobs(expires_at) WHERE expires_at IS NOT NULL;
