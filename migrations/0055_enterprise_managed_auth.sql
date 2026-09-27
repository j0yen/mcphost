-- compat: previous -- two wholly new tables; an old release simply never
-- queries either (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0055_enterprise_managed_auth: PRD-mcphost-enterprise-managed-auth
-- requirements 1, 3, 4. (0047-0050 were already claimed by in-flight/landed
-- branches at drafting time: mcphost-end-user-audit-and-revoke,
-- mcphost-runs-end-user-subject, mcphost-shared-call-run-scope, and
-- mcphost-hosted-authorization-server respectively; renumbered from this
-- PRD's own 0051, then 0052, then 0053, then 0054, during rebase --
-- mcphost-oauth-client-policy claimed 0051 first, then
-- mcphost-federated-end-user-login claimed 0052 first, then
-- mcphost-oauth-demand-signal claimed 0053 first, then
-- mcphost-tool-scopes-and-consent claimed 0054 first.)

-- `oauth_trusted_issuers`: one row per tenant-registered identity-assertion
-- issuer (requirement 1) -- `issuer` is globally unique, same one-tenant-
-- owns-an-issuer-string convention `oauth_issuers` (0038) already uses for
-- the bring-your-own-bearer path (a distinct registry: an identity
-- assertion's issuer is trusted for the JWT-bearer grant, never for a plain
-- bearer call). `client_id` is the pre-registered enterprise client allowed
-- to use the grant for this issuer (Anthropic's published id, or a CIMD
-- URL -- requirement 1); `audience`, when set, overrides the default
-- expected assertion `aud` (this host's own AS issuer URL) for a provider
-- that signs a different audience value.
CREATE TABLE IF NOT EXISTS oauth_trusted_issuers (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    tenant_id  INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    issuer     TEXT NOT NULL UNIQUE,
    jwks_url   TEXT NOT NULL,
    client_id  TEXT NOT NULL,
    audience   TEXT,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_oauth_trusted_issuers_tenant ON oauth_trusted_issuers(tenant_id);

-- `oauth_assertion_jti`: replay guard for identity assertions (requirement
-- 3/4: "replay window = assertion lifetime") -- the insert is itself the
-- atomic single-use claim (`INSERT ... ON CONFLICT DO NOTHING` + rows-
-- affected, same shape `Db::claim_oauth_code` already uses); `expires_unix`
-- mirrors the assertion's own `exp`, so a swept row is never reachable
-- again once the assertion it guarded could never validate anyway.
CREATE TABLE IF NOT EXISTS oauth_assertion_jti (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    issuer       TEXT NOT NULL,
    jti          TEXT NOT NULL,
    expires_unix INTEGER NOT NULL,
    created_unix INTEGER NOT NULL,
    UNIQUE (issuer, jti)
);
