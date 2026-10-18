-- compat: previous -- two nullable columns plus two wholly new tables; an old release never reads them.
-- mcphost 0124_owner_verification_method: PRD-mcphost-owner-verification-paths R2.
--
-- owner_method: how the owner proved a human stands behind the tenant, set in
-- the same UPDATE as owner_verified_at. The CHECK set is the
-- VERIFICATION_METHODS names in src/verify.rs (a drift test compares them).
-- owner_handle: the GitHub login or the DNS domain that proved it.
ALTER TABLE tenants ADD COLUMN owner_method TEXT CHECK (owner_method IN ('email', 'github', 'dns'));
ALTER TABLE tenants ADD COLUMN owner_handle TEXT;
UPDATE tenants SET owner_method = 'email' WHERE owner_verified_at IS NOT NULL AND owner_method IS NULL;

-- One row per outstanding GitHub authorize redirect: the CSRF state, 10 min TTL.
CREATE TABLE IF NOT EXISTS owner_github_states (
    state        TEXT PRIMARY KEY,
    tenant_id    INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    expires_unix INTEGER NOT NULL
);

-- One row per (tenant, domain) DNS challenge: the 32-byte token the TXT record carries.
CREATE TABLE IF NOT EXISTS owner_dns_challenges (
    tenant_id    INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    domain       TEXT NOT NULL,
    token        TEXT NOT NULL,
    created_unix INTEGER NOT NULL,
    PRIMARY KEY (tenant_id, domain)
);
