-- compat: previous -- additive nullable columns and new tables; an old release never reads them.
-- mcphost 0120_owner_verification_method: PRD-mcphost-owner-verification-paths R2.
--
-- owner_method: how the owner proved a human stands behind the tenant, set in
-- the same single-winner UPDATE as owner_verified_at. The CHECK list is
-- `verify::VERIFICATION_METHODS` (a drift test asserts they match).
-- owner_handle: the GitHub login or the DNS domain (NULL for email).
ALTER TABLE tenants ADD COLUMN owner_method TEXT CHECK (owner_method IN ('email', 'github', 'dns'));
ALTER TABLE tenants ADD COLUMN owner_handle TEXT;
UPDATE tenants SET owner_method = 'email' WHERE owner_verified_at IS NOT NULL AND owner_method IS NULL;

-- R3: one row per outstanding GitHub authorize (CSRF state), 10 minute TTL.
CREATE TABLE IF NOT EXISTS claim_github_states (
    state_hash   TEXT PRIMARY KEY,
    tenant_id    INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    expires_unix INTEGER NOT NULL,
    created_unix INTEGER NOT NULL
);

-- R4: the per-tenant, per-domain TXT token.
CREATE TABLE IF NOT EXISTS claim_dns_tokens (
    tenant_id    INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    domain       TEXT NOT NULL,
    token        TEXT NOT NULL,
    created_unix INTEGER NOT NULL,
    PRIMARY KEY (tenant_id, domain)
);
