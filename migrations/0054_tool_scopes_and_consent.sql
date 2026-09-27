-- compat: previous -- one new table (`oauth_scopes`) plus two additive
-- columns (`tools.scopes`, `oauth_grants.scope`), each with a default that
-- makes every existing row read exactly as it did before this PRD
-- (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0054_tool_scopes_and_consent: PRD-mcphost-tool-scopes-and-consent
-- requirements 1-6.

-- `oauth_scopes`: a tenant's own scope catalog (requirement 1) --
-- `host.oauth.scope_set {name, description}` upserts a row here; a tool's
-- `scopes` may name any catalogued name here, or the built-ins `read`/
-- `write`, which need no row (requirement 1: "a tool may reference only
-- catalogued scopes or the built-ins read, write" -- the built-ins are
-- always valid, catalogued only for their consent-page description).
CREATE TABLE IF NOT EXISTS oauth_scopes (
    id           INTEGER PRIMARY KEY,
    tenant_id    INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    description  TEXT NOT NULL,
    created_unix INTEGER NOT NULL,
    UNIQUE(tenant_id, name)
);
CREATE INDEX IF NOT EXISTS idx_oauth_scopes_tenant ON oauth_scopes(tenant_id);

-- `tools.scopes`: a JSON array of scope names (TEXT, same convention as
-- `oauth_clients.redirect_uris`) -- absent/empty means the tool requires
-- only `mcp` (requirement 1). Set fresh at every `host.tool_publish`, not
-- carried over from a prior version (unlike `kind`/`spec`, scopes are not
-- versioned history -- requirement 5/AC6: changing it takes effect on the
-- tenant's live `tools` row immediately, no token reissue needed).
ALTER TABLE tools ADD COLUMN scopes TEXT NOT NULL DEFAULT '[]';

-- `oauth_grants.scope`: the space-separated scope string this grant was
-- actually issued with (requirement 6: "host.oauth.grants rows show the
-- granted scopes"); every pre-existing grant defaults to `mcp`, exactly
-- what `issue_tokens` always minted before this PRD.
ALTER TABLE oauth_grants ADD COLUMN scope TEXT NOT NULL DEFAULT 'mcp';
