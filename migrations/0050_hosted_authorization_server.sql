-- compat: previous -- six wholly new tables; an old release simply never
-- queries them (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0050_hosted_authorization_server: PRD-mcphost-hosted-authorization-server
-- requirements 2-7. (0047/0048/0049 were already claimed by in-flight
-- branches at drafting time: mcphost-end-user-audit-and-revoke,
-- mcphost-runs-end-user-subject, and mcphost-shared-call-run-scope
-- respectively.)

-- `oauth_clients`: DCR-registered clients only (RFC 7591, requirement
-- 2(b)). A CIMD client's identity is its own `https` URL, fetched and
-- validated on demand (`authz::CimdCache`) -- never a row here, so a CIMD
-- client can never collide with, or be confused for, a DCR registration.
-- `client_secret_hash` is `NULL` for a public client
-- (`token_endpoint_auth_method = "none"`, the only shape a `native` app can
-- be); when present it's sha256 hex, same hash-at-rest convention as
-- `tenants.key_hash`. `redirect_uris` is a JSON array (TEXT) -- this
-- table's one column that isn't a scalar, same convention
-- `runs.counters_json` already uses for a small bounded array/object.
CREATE TABLE IF NOT EXISTS oauth_clients (
    id                         INTEGER PRIMARY KEY,
    client_id                  TEXT NOT NULL UNIQUE,
    client_secret_hash         TEXT,
    client_name                TEXT,
    redirect_uris              TEXT NOT NULL,
    application_type           TEXT NOT NULL,
    token_endpoint_auth_method TEXT NOT NULL,
    created_unix               INTEGER NOT NULL
);

-- `oauth_register_events`: requirement 2(b)'s 10-per-minute-per-IP `POST
-- /oauth/register` limit -- same atomic check-and-insert-in-one-statement
-- shape as `signup_events`/`Db::try_admit_signup`.
CREATE TABLE IF NOT EXISTS oauth_register_events (
    id           INTEGER PRIMARY KEY,
    source_ip    TEXT NOT NULL,
    created_unix INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_oauth_register_events_ip_created ON oauth_register_events(source_ip, created_unix);

-- `oauth_grants`: one row per completed authorization -- a consent ->
-- code -> token exchange that actually minted tokens. The unit
-- `host.oauth.grants`/`host.oauth.grant_revoke` (requirement 7) and `POST
-- /oauth/revoke` (RFC 7009) operate on; `client_name`/`method` are copied
-- from the identified client at grant-creation time (not joined from
-- `oauth_clients` at read time), so a since-deleted or re-fetched-differently
-- CIMD document never changes what an already-issued grant is shown as.
CREATE TABLE IF NOT EXISTS oauth_grants (
    id             INTEGER PRIMARY KEY,
    tenant_id      INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    client_id      TEXT NOT NULL,
    client_name    TEXT,
    method         TEXT NOT NULL,
    resource       TEXT NOT NULL,
    created_unix   INTEGER NOT NULL,
    last_used_unix INTEGER NOT NULL,
    revoked_unix   INTEGER
);
CREATE INDEX IF NOT EXISTS idx_oauth_grants_tenant ON oauth_grants(tenant_id);

-- `oauth_codes`: single-use authorization codes, 60s TTL (requirement 3/4).
-- `code_hash` is the only form ever stored at rest, same convention as
-- `claim_codes.code_hash`/`vault_handoff_tokens.token_hash`. `grant_id` is
-- `NULL` until this code's first successful redemption at `POST
-- /oauth/token`; recording it there (not before) is what lets a replay of
-- an already-consumed code (AC5/AC7) look up and revoke exactly the
-- grant/tokens that first, and only legitimate, redemption minted.
CREATE TABLE IF NOT EXISTS oauth_codes (
    id             INTEGER PRIMARY KEY,
    code_hash      TEXT NOT NULL UNIQUE,
    tenant_id      INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    client_id      TEXT NOT NULL,
    client_name    TEXT,
    method         TEXT NOT NULL,
    redirect_uri   TEXT NOT NULL,
    code_challenge TEXT NOT NULL,
    resource       TEXT NOT NULL,
    scope          TEXT NOT NULL,
    grant_id       INTEGER REFERENCES oauth_grants(id) ON DELETE SET NULL,
    expires_unix   INTEGER NOT NULL,
    consumed_unix  INTEGER,
    created_unix   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_oauth_codes_tenant ON oauth_codes(tenant_id);

-- `oauth_refresh_tokens`: a rotating chain, one row per issued refresh
-- token (`token_hash` hashed at rest, 30d TTL, requirement 4).
-- `rotated_unix` marks single-use -- set the instant this token is
-- exchanged for the next one in the chain. Reuse of an already-rotated
-- token (AC7) revokes the whole `grant_id`, not just this row.
CREATE TABLE IF NOT EXISTS oauth_refresh_tokens (
    id           INTEGER PRIMARY KEY,
    grant_id     INTEGER NOT NULL REFERENCES oauth_grants(id) ON DELETE CASCADE,
    token_hash   TEXT NOT NULL UNIQUE,
    resource     TEXT NOT NULL,
    expires_unix INTEGER NOT NULL,
    rotated_unix INTEGER,
    created_unix INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_oauth_refresh_tokens_grant ON oauth_refresh_tokens(grant_id);

-- `oauth_jti_denylist`: every hosted access token's `jti`, recorded at
-- mint time alongside the JWT itself -- membership alone isn't "denied";
-- `denied_unix` being set is what actually marks a jti revoked (AC8's "jti
-- denylist" is this column, not the table's mere existence). Doubling as
-- the mint-time registry (rather than a second, separate table) is what
-- lets a grant revocation or a refresh-token-reuse event look up exactly
-- which jtis to deny.
CREATE TABLE IF NOT EXISTS oauth_jti_denylist (
    id           INTEGER PRIMARY KEY,
    jti          TEXT NOT NULL UNIQUE,
    grant_id     INTEGER NOT NULL REFERENCES oauth_grants(id) ON DELETE CASCADE,
    tenant_id    INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    resource     TEXT NOT NULL,
    expires_unix INTEGER NOT NULL,
    denied_unix  INTEGER,
    created_unix INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_oauth_jti_denylist_grant ON oauth_jti_denylist(grant_id);
