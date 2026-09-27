-- compat: previous -- six wholly new tables; an old release simply never
-- queries them (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0051_oauth_client_policy: PRD-mcphost-oauth-client-policy
-- requirements 1-6.

-- `oauth_policies`: one row per tenant, only ever written by
-- `host.oauth.policy_set` -- a tenant with no row here gets this PRD's
-- documented defaults (requirement 1: "defaults reproduce the hosted AS
-- PRD's behaviour exactly"), read entirely in Rust
-- (`oauth_policy::OauthPolicy::default_for`) rather than as SQL column
-- defaults, so the default logic lives in exactly one place.
-- `allowlist_json` is a JSON array (TEXT) of either a DCR `client_id` or a
-- CIMD host string, same "one small bounded array as a TEXT column"
-- convention `oauth_clients.redirect_uris` already uses.
CREATE TABLE IF NOT EXISTS oauth_policies (
    tenant_id         INTEGER PRIMARY KEY REFERENCES tenants(id) ON DELETE CASCADE,
    clients_mode      TEXT NOT NULL,
    allowlist_json    TEXT NOT NULL,
    access_ttl_s      INTEGER NOT NULL,
    refresh_ttl_s     INTEGER NOT NULL,
    max_grant_age_s   INTEGER,
    reconsent_after_s INTEGER,
    updated_unix      INTEGER NOT NULL
);

-- `oauth_pending_clients`: requirement 2's `clients: approve` queue -- a
-- client seen at `/oauth/authorize` under `approve` mode that is neither
-- allowlisted nor already approved gets one row here until
-- `host.oauth.client_approve`/`host.oauth.client_deny` resolves it.
CREATE TABLE IF NOT EXISTS oauth_pending_clients (
    id           INTEGER PRIMARY KEY,
    tenant_id    INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    client_id    TEXT NOT NULL,
    client_name  TEXT,
    method       TEXT NOT NULL,
    created_unix INTEGER NOT NULL,
    UNIQUE (tenant_id, client_id)
);

-- `oauth_approved_clients`: a client this tenant's `host.oauth.client_approve`
-- has ever accepted -- checked ahead of `oauth_pending_clients` so an
-- already-approved client's next authorize reaches consent directly
-- instead of being re-queued. Deliberately untouched by
-- `host.oauth.revoke_all` (requirement 3: that revokes live grants and the
-- pending queue, not this tenant's standing approve decisions).
CREATE TABLE IF NOT EXISTS oauth_approved_clients (
    id            INTEGER PRIMARY KEY,
    tenant_id     INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    client_id     TEXT NOT NULL,
    approved_unix INTEGER NOT NULL,
    UNIQUE (tenant_id, client_id)
);

-- `oauth_denied_clients`: requirement 2's `host.oauth.client_deny` --
-- distinct from simply removing the `oauth_pending_clients` row (which
-- alone would let the same client re-queue itself as pending on its very
-- next attempt): a denied client must keep reading `client_not_allowed`
-- indefinitely, so denial needs its own durable row, checked ahead of
-- `approve` mode's pending-queue logic.
CREATE TABLE IF NOT EXISTS oauth_denied_clients (
    id           INTEGER PRIMARY KEY,
    tenant_id    INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    client_id    TEXT NOT NULL,
    denied_unix  INTEGER NOT NULL,
    UNIQUE (tenant_id, client_id)
);

-- `oauth_audit`: requirement 4's tenant-readable auth audit trail --
-- `tenant_id` is nullable because a handful of events (an operator block
-- refused before any tenant is even identified, e.g. at `/oauth/authorize`
-- before consent) have no tenant to attribute to yet; every tenant-facing
-- read (`host.oauth.audit`/`host.oauth.audit_export`) filters
-- `tenant_id = ?` so a `NULL` row never surfaces in any tenant's own log.
-- `ip_hash` is a salted hash (`SecretBox::salted_hash`, salt derived from
-- the same passphrase-derived key `SecretBox` already holds) -- the raw
-- address is never stored (Technical considerations).
CREATE TABLE IF NOT EXISTS oauth_audit (
    id               INTEGER PRIMARY KEY,
    tenant_id        INTEGER REFERENCES tenants(id) ON DELETE CASCADE,
    ts               INTEGER NOT NULL,
    event            TEXT NOT NULL,
    client_id        TEXT,
    method           TEXT,
    end_user_subject TEXT,
    reason           TEXT,
    ip_hash          TEXT,
    scopes           TEXT
);
CREATE INDEX IF NOT EXISTS idx_oauth_audit_tenant_ts ON oauth_audit(tenant_id, ts);

-- `oauth_client_blocks`: requirement 5's operator-global block list,
-- checked ahead of any tenant's own policy (Technical considerations: "the
-- operator block list is checked before any tenant policy"). `kind` is
-- `client_id` or `cimd_host`; `UNIQUE(kind, value)` makes
-- `admin.oauth.client_block` idempotent on a repeat call for the same
-- target.
CREATE TABLE IF NOT EXISTS oauth_client_blocks (
    id           INTEGER PRIMARY KEY,
    kind         TEXT NOT NULL,
    value        TEXT NOT NULL,
    reason       TEXT,
    created_unix INTEGER NOT NULL,
    UNIQUE (kind, value)
);

-- `oauth_cimd_register_events`: requirement 6's "≤ 100 registrations per
-- CIMD host per day, host-wide" -- a CIMD client (never a
-- `POST /oauth/register` caller; it has no DCR row at all) is counted here
-- the first time each `/oauth/authorize` attempt identifies it, same
-- atomic check-and-insert shape `oauth_register_events`
-- (`Db::try_admit_oauth_register`) already uses for the DCR-per-IP limit,
-- keyed by CIMD host instead of source IP since a CIMD fetch is made by
-- this host itself, never the attacker's own IP.
CREATE TABLE IF NOT EXISTS oauth_cimd_register_events (
    id           INTEGER PRIMARY KEY,
    cimd_host    TEXT NOT NULL,
    created_unix INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_oauth_cimd_register_events_host_created ON oauth_cimd_register_events(cimd_host, created_unix);
