-- compat: previous -- four additive nullable columns on `tenants` plus two
-- wholly new tables; an old release simply never queries any of them, no
-- existing row's shape changes, no existing statement's result set changes
-- (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0034_claim: PRD-mcphost-human-claim-magic-link requirements 1-7.
--
-- `tenants.claim_token_hash`/`claim_expires_at`: the single active claim
-- token `signup`'s `claim_url` names, sha256-hashed (same convention as
-- `key_hash`/`handoff_tokens.token_hash`) -- never stored in the clear.
-- One active token per tenant, on the tenant row itself (not a table of
-- its own), since a tenant only ever has one outstanding claim link at a
-- time; `Db::verify_claim_code` clears both columns the moment the tenant
-- is actually claimed.
--
-- `tenants.owner_email`/`owner_verified_at`: set together, exactly once,
-- by `Db::verify_claim_code`'s atomic `UPDATE ... WHERE owner_verified_at
-- IS NULL` (requirement 3 / AC7's single-winner race) -- never touched by
-- any other statement.
ALTER TABLE tenants ADD COLUMN owner_email TEXT;
ALTER TABLE tenants ADD COLUMN owner_verified_at INTEGER;
ALTER TABLE tenants ADD COLUMN claim_token_hash TEXT;
ALTER TABLE tenants ADD COLUMN claim_expires_at INTEGER;

CREATE INDEX IF NOT EXISTS idx_tenants_claim_token_hash ON tenants(claim_token_hash);

-- `claim_codes`: one row per magic-link send, `code_hash` (sha256 hex) the
-- only form of the verify code ever stored -- same "hash at rest,
-- single-use via an `UPDATE ... WHERE consumed_unix IS NULL` claim" shape
-- as `handoff_tokens` (migration 0019). Requirement 7's "3 sends per
-- tenant per hour" is enforced by `Db::try_create_claim_code`'s own atomic
-- check-and-insert (same pattern as `Db::try_admit_signup`), counting rows
-- here by `tenant_id`/`created_unix`, so no separate counter table is
-- needed.
CREATE TABLE IF NOT EXISTS claim_codes (
    id            INTEGER PRIMARY KEY,
    tenant_id     INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    code_hash     TEXT NOT NULL UNIQUE,
    email         TEXT NOT NULL,
    expires_unix  INTEGER NOT NULL,
    consumed_unix INTEGER,
    created_unix  INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_claim_codes_tenant_created ON claim_codes(tenant_id, created_unix);

-- `claim_rate_events`: requirement 7's per-IP 30/hour limit on
-- `GET`/`POST /claim/*` -- same shape and same atomic
-- check-and-insert-in-one-statement pattern as `signup_events`/
-- `Db::try_admit_signup`, just for this route family instead of `signup`.
CREATE TABLE IF NOT EXISTS claim_rate_events (
    source_ip    TEXT NOT NULL,
    created_unix INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_claim_rate_events_ip_created ON claim_rate_events(source_ip, created_unix);
