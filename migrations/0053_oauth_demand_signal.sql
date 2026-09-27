-- compat: previous -- one additive column with a default (an old release
-- simply never reads it, and its own inserts still succeed since it never
-- names the column) plus two wholly new tables (PRD-mcphost-migration-safety
-- requirement 4).
-- mcphost 0053_oauth_demand_signal: PRD-mcphost-oauth-demand-signal
-- requirements 1-3.

-- requirement 1 (AC1): which credential kind resolved this call --
-- `key` (a tenant key, header or `tenant_key` argument), `issuer_jwt` (a
-- tenant-registered bring-your-own issuer bearer), or `hosted_token` (this
-- host's own built-in authorization server). `DEFAULT 'key'` backfills
-- every pre-migration row for free: a row written before this column
-- existed was, by construction, never anything but a key-authenticated
-- call (`issuer_jwt`/`hosted_token` are both brand new as of
-- PRD-mcphost-oauth-resource-server / PRD-mcphost-hosted-authorization-server).
ALTER TABLE calls ADD COLUMN auth_method TEXT NOT NULL DEFAULT 'key';

-- requirement 2: admin healthz's `oauth.calls_7d`/`calls_30d` (every call,
-- any tenant) and `oauth.tenants_7d`/`tenants_30d` (distinct non-synthetic
-- tenants, joined against `tenants.synthetic`) are both single indexed
-- scans over this shape -- `(auth_method, started_unix)` serves the
-- unqualified per-method count, `(tenant_id, auth_method, started_unix)`
-- serves the per-tenant-per-method rows `admin.oauth.stats` groups by.
CREATE INDEX IF NOT EXISTS idx_calls_auth_method_started ON calls(auth_method, started_unix);
CREATE INDEX IF NOT EXISTS idx_calls_tenant_auth_method_started ON calls(tenant_id, auth_method, started_unix);

-- requirement 3: a CIMD client has no `oauth_clients` row (migration 0050's
-- own comment: "never a row here") -- this table is this PRD's own
-- first-seen ledger for one, so `admin.oauth.stats`'s `clients.cimd` and
-- the registration funnel's `registrations_7d.cimd` have something to
-- count. `INSERT OR IGNORE`d from `authz::identify_client`'s cimd branch on
-- every resolution (cheap: at most one row per distinct CIMD `client_id`
-- ever seen), so `created_unix` is always this client's first-ever
-- identification, never its most recent.
CREATE TABLE IF NOT EXISTS oauth_cimd_clients (
    client_id    TEXT PRIMARY KEY,
    created_unix INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_oauth_cimd_clients_created ON oauth_cimd_clients(created_unix);

-- requirement 3: the registration/consent/token funnel's own event log --
-- `authorize_request` (every `GET`/`POST /oauth/authorize` hit, before any
-- validation, so a caller can see the drop-off between "showed up" and
-- "consented"), `consent` (a code was actually minted), `token_issued`
-- (a `POST /oauth/token` exchange -- either grant type -- actually minted a
-- token). No `method`/`tenant_id` column: the funnel's own fields
-- (`authorize_requests_7d`/`consents_7d`/`tokens_issued_7d`) are host-wide
-- scalars, not split by method or tenant, unlike `registrations_7d`
-- (`oauth_clients`/`oauth_cimd_clients` already carry that split).
CREATE TABLE IF NOT EXISTS oauth_funnel_events (
    id           INTEGER PRIMARY KEY,
    event        TEXT NOT NULL,
    created_unix INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_oauth_funnel_events_event_created ON oauth_funnel_events(event, created_unix);
