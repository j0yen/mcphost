-- compat: previous -- three wholly new tables; an old release simply never
-- queries them (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0046_vault: PRD-mcphost-upstream-token-vault requirement 1.
-- (Renumbered from the PRD's drafted "migration 0040", then again from
-- 0045 during the mcphost-run-result-overflow-to-state rebase -- both
-- numbers were already claimed by 0040_alerts.sql and
-- 0045_run_counters_and_error_data.sql respectively by the time this PRD
-- reached build.)
--
-- `vault_providers` is a tenant's registered upstream OAuth application
-- (`host.vault.provider_set`) -- one row per `(tenant_id, name)`.
-- `client_secret_enc`/`client_secret_nonce` reuse `secrets.rs`'s
-- AES-256-GCM at-rest scheme (`AppState::secrets`), the same convention
-- `secrets.value_enc`/`nonce` already uses for tenant secrets -- never
-- plaintext at rest, and never returned by `host.vault.providers`.
CREATE TABLE IF NOT EXISTS vault_providers (
    id                   INTEGER PRIMARY KEY,
    tenant_id            INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    name                 TEXT NOT NULL,
    auth_url             TEXT NOT NULL,
    token_url            TEXT NOT NULL,
    client_id            TEXT NOT NULL,
    client_secret_enc    BLOB NOT NULL,
    client_secret_nonce  BLOB NOT NULL,
    scopes               TEXT NOT NULL,
    created_unix         INTEGER NOT NULL,
    UNIQUE(tenant_id, name)
);

-- `vault_tokens` is one end user's stored upstream token for one tenant's
-- provider -- one row per `(tenant_id, provider, end_user_subject)`.
-- `access_enc`/`refresh_enc` (plus their nonces) are the same
-- `AppState::secrets`-encrypted scheme as `vault_providers.client_secret_enc`
-- above -- requirement 4's "the token never appears in a tool result, a
-- log, or a model context" starts with it never being stored in the clear
-- either. `refresh_enc`/`refresh_nonce` are nullable: a provider may issue
-- an access token with no refresh token at all. `revoked_unix`/
-- `revoked_reason` are set by `host.vault.disconnect` (AC8) or a failed
-- refresh (P1 requirement 6 / AC10) -- a revoked row is never treated as
-- connected again; the end user must redo the connect-link flow.
CREATE TABLE IF NOT EXISTS vault_tokens (
    id                   INTEGER PRIMARY KEY,
    tenant_id            INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    provider             TEXT NOT NULL,
    end_user_subject     TEXT NOT NULL,
    end_user_issuer      TEXT,
    access_enc           BLOB NOT NULL,
    access_nonce         BLOB NOT NULL,
    refresh_enc          BLOB,
    refresh_nonce        BLOB,
    expires_unix         INTEGER NOT NULL,
    scopes               TEXT NOT NULL,
    connected_unix       INTEGER NOT NULL,
    last_refreshed_unix  INTEGER,
    revoked_unix         INTEGER,
    revoked_reason       TEXT,
    UNIQUE(tenant_id, provider, end_user_subject)
);
CREATE INDEX IF NOT EXISTS idx_vault_tokens_lookup
    ON vault_tokens(tenant_id, provider, end_user_subject);

-- `vault_handoff_tokens` is the one-time `/vault/connect/<token>` link's
-- own row -- same single-use shape as `handoff_tokens` (migration 0019:
-- `token_hash` is the only form of the raw token ever stored, redemption is
-- an `UPDATE ... WHERE redeemed_unix IS NULL`), but a distinct table rather
-- than a reuse of `handoff_tokens` itself: that table's `key_enc`/`key_nonce`
-- shape is specific to `host.redeem`'s tenant-key handoff (0019's own
-- migration note) and carries no provider/end-user/PKCE columns this flow
-- needs. `pkce_verifier_enc`/`pkce_verifier_nonce`: the PKCE code verifier,
-- generated once at `host.vault.connect_link` time and encrypted at rest
-- (Technical considerations: "PKCE verifier stored with the handoff token,
-- encrypted"), decrypted only inside the `/vault/connect/<token>` and
-- `/vault/callback` handlers. `oauth_state` is a second, independent random
-- value (never the raw connect token) sent as the OAuth `state` parameter --
-- binds the provider's callback back to this handoff without putting the
-- one-time connect token itself in a URL a third-party auth server, and the
-- end user's browser history, would also see.
CREATE TABLE IF NOT EXISTS vault_handoff_tokens (
    id                    INTEGER PRIMARY KEY,
    tenant_id             INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    token_hash            TEXT NOT NULL UNIQUE,
    provider              TEXT NOT NULL,
    end_user_subject      TEXT NOT NULL,
    end_user_issuer       TEXT,
    pkce_verifier_enc     BLOB NOT NULL,
    pkce_verifier_nonce   BLOB NOT NULL,
    oauth_state           TEXT NOT NULL UNIQUE,
    expires_unix          INTEGER NOT NULL,
    redeemed_unix         INTEGER,
    created_unix          INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_vault_handoff_tokens_tenant ON vault_handoff_tokens(tenant_id);
