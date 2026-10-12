-- compat: previous -- one wholly new table and one nullable column on
-- `tenants` (`scratch_of`); an old release never selects it and its
-- `INSERT`s name their columns, so no statement's result set changes
-- (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0092_confirm_tokens: PRD-mcphost-destructive-verb-confirmation-token R2/R5.
--
-- `confirm_tokens` holds one row per minted `confirm` token, keyed by the
-- SHA-256 of the token (the token itself is never stored). A token is bound
-- to tenant, verb, canonical-args hash and diff hash; `used_unix` is set in
-- the same transaction as the write it authorises.
CREATE TABLE IF NOT EXISTS confirm_tokens (
    token_hash   TEXT PRIMARY KEY,
    tenant_id    INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    verb         TEXT NOT NULL,
    args_hash    TEXT NOT NULL,
    diff_hash    TEXT NOT NULL,
    expires_unix INTEGER NOT NULL,
    used_unix    INTEGER
);
CREATE INDEX IF NOT EXISTS confirm_tokens_expiry ON confirm_tokens(expires_unix);
-- A scratch tenant (`scratch_of` = its parent's id) is never confirm-gated (R5).
ALTER TABLE tenants ADD COLUMN scratch_of INTEGER;
