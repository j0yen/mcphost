-- compat: previous -- two wholly new tables plus additive columns; an old
-- release simply never queries them (PRD-mcphost-migration-safety
-- requirement 4).
-- mcphost 0052_federated_end_user_login: PRD-mcphost-federated-end-user-login
-- requirements 1-6.

-- `oauth_providers`: one tenant's own OIDC identity provider -- the
-- browser-login sibling of `oauth_issuers`' bring-your-own-issuer bearer
-- flow, at most one per tenant. `client_secret_enc`/`client_secret_nonce`
-- are `AppState::secrets`-encrypted, same convention as
-- `vault_providers.client_secret_enc`. `authorization_endpoint`/
-- `token_endpoint`/`jwks_uri` are fetched once from the issuer's discovery
-- document at `host.oauth.provider_set` time and cached here -- never
-- re-fetched per authorize/token, so a discovery outage after setup never
-- blocks a login the provider itself is still up for.
CREATE TABLE IF NOT EXISTS oauth_providers (
    id                     INTEGER PRIMARY KEY,
    tenant_id              INTEGER NOT NULL UNIQUE REFERENCES tenants(id) ON DELETE CASCADE,
    issuer                 TEXT NOT NULL,
    client_id              TEXT NOT NULL,
    client_secret_enc      BLOB NOT NULL,
    client_secret_nonce    BLOB NOT NULL,
    scopes                 TEXT NOT NULL,
    claims_map_json        TEXT,
    authorization_endpoint TEXT NOT NULL,
    token_endpoint         TEXT NOT NULL,
    jwks_uri               TEXT NOT NULL,
    require_verified_email INTEGER NOT NULL DEFAULT 1,
    owner_login            INTEGER NOT NULL DEFAULT 0,
    created_unix           INTEGER NOT NULL
);

-- `oauth_federation_pending`: one row per in-flight upstream round trip,
-- from `GET /oauth/authorize` redirecting to the provider through to the
-- browser approving mcphost's own consent page (requirements 2/6).
-- Non-functional: capped at 1,000 rows per tenant, 10-minute expiry.
-- `status` is `pending` until `GET /oauth/federation/callback` validates
-- the provider's `id_token`, then `verified` -- the browser's later
-- consent-approval `POST` requires `verified`, never mints a code from a
-- still-`pending` row.
CREATE TABLE IF NOT EXISTS oauth_federation_pending (
    id               INTEGER PRIMARY KEY,
    upstream_state   TEXT NOT NULL UNIQUE,
    tenant_id        INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    client_id        TEXT NOT NULL,
    client_name      TEXT,
    method           TEXT NOT NULL,
    redirect_uri     TEXT NOT NULL,
    code_challenge   TEXT NOT NULL,
    resource         TEXT NOT NULL,
    scope            TEXT NOT NULL,
    original_state   TEXT NOT NULL,
    nonce            TEXT NOT NULL,
    pkce_verifier    TEXT NOT NULL,
    status           TEXT NOT NULL,
    end_user_subject TEXT,
    end_user_email   TEXT,
    end_user_name    TEXT,
    end_user_issuer  TEXT,
    created_unix     INTEGER NOT NULL,
    expires_unix     INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_oauth_federation_pending_tenant ON oauth_federation_pending(tenant_id);

-- requirement 3/5: a federated grant's/code's end user -- `NULL` for the
-- pre-existing tenant-owner consent path (every row minted before this
-- PRD, and every non-federated row minted after it). `end_user_subject` is
-- already namespaced by the provider issuer (`crate::federation`) so it
-- never needs its own issuer column to stay collision-free across two
-- providers; `end_user_issuer` is kept anyway so `EndUser.issuer` doesn't
-- have to reverse the namespacing scheme.
ALTER TABLE oauth_codes ADD COLUMN end_user_subject TEXT;
ALTER TABLE oauth_codes ADD COLUMN end_user_email TEXT;
ALTER TABLE oauth_codes ADD COLUMN end_user_name TEXT;
ALTER TABLE oauth_codes ADD COLUMN end_user_issuer TEXT;

ALTER TABLE oauth_grants ADD COLUMN end_user_subject TEXT;
ALTER TABLE oauth_grants ADD COLUMN end_user_email TEXT;
ALTER TABLE oauth_grants ADD COLUMN end_user_name TEXT;
ALTER TABLE oauth_grants ADD COLUMN end_user_issuer TEXT;
CREATE INDEX IF NOT EXISTS idx_oauth_grants_end_user ON oauth_grants(tenant_id, end_user_subject);
