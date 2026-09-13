-- compat: previous -- one wholly new table plus one additive column an old
-- release simply never queries; no existing table's shape changes
-- (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0019_handoff_token: PRD-mcphost-handoff-token requirements 1-3.
--
-- `handoff_tokens` is `signup(handoff: true)`'s single-use exchange row:
-- `token_hash` (sha256 hex, same convention as `tenants.key_hash`) is the
-- only form of the token ever stored. The raw tenant key `host.redeem`
-- hands back is stored AES-256-GCM-encrypted (`key_enc`/`key_nonce`) under
-- the same passphrase-derived cipher `host.secret_set` already uses for
-- tenant secrets (see `src/secrets.rs::SecretBox`) -- never plaintext at
-- rest, and decrypted exactly once, in `control::redeem`, right before the
-- row is marked redeemed. `redeemed_unix IS NULL` is the single-use gate:
-- `Db::redeem_handoff_token` claims it with an
-- `UPDATE ... WHERE redeemed_unix IS NULL` so two concurrent redemptions of
-- the same token can't both win.
--
-- New table, like migration 0011/0014/0015's own tables -- `ON DELETE
-- CASCADE` on `tenant_id` needs no surgery, just part of the `CREATE
-- TABLE`.
--
-- `tenants.key_rotated_unix` (requirement 3 / P1 requirement 7): `NULL`
-- until the first `host.key_rotate`, then that rotation's own unix
-- timestamp -- `host.whoami`'s `key_age_s` falls back to `created_unix`
-- when this is `NULL` (a key that has never been rotated is exactly as old
-- as the tenant itself).
CREATE TABLE IF NOT EXISTS handoff_tokens (
    id            INTEGER PRIMARY KEY,
    tenant_id     INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    token_hash    TEXT NOT NULL UNIQUE,
    key_enc       BLOB NOT NULL,
    key_nonce     BLOB NOT NULL,
    expires_unix  INTEGER NOT NULL,
    redeemed_unix INTEGER,
    created_unix  INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_handoff_tokens_tenant ON handoff_tokens(tenant_id);

ALTER TABLE tenants ADD COLUMN key_rotated_unix INTEGER;
