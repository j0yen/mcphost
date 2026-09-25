-- compat: previous -- two wholly new tables; an old release simply never
-- queries either, no existing row's shape changes, no existing statement's
-- result set changes (PRD-mcphost-migration-safety requirement 4, same
-- convention 0035/0036's own compat notes already follow).
-- mcphost 0038_oauth_issuers: PRD-mcphost-oauth-resource-server requirement 3.
--
-- `oauth_issuers`: one row per tenant-registered issuer -- `issuer` is
-- globally unique (requirement 3: "an issuer may be registered by only one
-- tenant"), enforced by the UNIQUE constraint rather than an app-level
-- check-then-insert race. `jwks_json`/`last_jwks_at` mirror the most
-- recent successful JWKS fetch (requirement 4's refetch-on-unknown-kid
-- path writes both back here) so `admin.oauth.issuers`' JWKS age survives
-- a restart, unlike the in-process `oauth::JwksCache` this crate also
-- keeps for hot-path lookups.
CREATE TABLE IF NOT EXISTS oauth_issuers (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    tenant_id    INTEGER NOT NULL,
    issuer       TEXT NOT NULL UNIQUE,
    audience     TEXT NOT NULL,
    jwks_url     TEXT NOT NULL,
    created_at   TEXT NOT NULL,
    last_jwks_at INTEGER,
    jwks_json    TEXT
);

CREATE INDEX IF NOT EXISTS idx_oauth_issuers_tenant ON oauth_issuers(tenant_id);

-- `oauth_rejections`: per-issuer, per-reason lifetime rejection counters
-- (requirement 5 / AC3, AC9) -- keyed by the issuer STRING (not
-- oauth_issuers.id) so a rejection for an issuer string that matched no
-- registered row (unknown_issuer) still has somewhere to count, and a
-- later host.oauth.issuer_set for that same string keeps whatever counts
-- predate the registration.
CREATE TABLE IF NOT EXISTS oauth_rejections (
    issuer TEXT NOT NULL,
    reason TEXT NOT NULL,
    count  INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (issuer, reason)
);
