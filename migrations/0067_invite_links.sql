-- compat: previous -- two wholly new tables plus two additive nullable
-- columns (one on `tenants`, one on `contacts`); no existing column is
-- dropped or narrowed, no existing row's shape changes.
-- mcphost 0067_invite_links: PRD-mcphost-invite-links requirements 1-6,
-- 10-13.
--
-- `invites`: one row per invite link (`kind = 'standard'`, created by
-- `host.invite.create`, capped/expiring/revocable) or standing invite
-- (`kind = 'standing'`, one per tenant, created at tenant birth or lazily
-- on first `host.whoami`, no expiry, no `max_uses`). `code_hash` is the
-- SHA-256 hex of the plaintext code (`auth::generate_url_secret`'s own
-- 128-bit/26-char shape, reused here -- non-functional clause: "a code is
-- 20+ characters of 100+ bits"), never the plaintext itself, same
-- hash-at-rest convention as `tenants.key_hash`/`url_secret_hash`.
-- `share_json` is a JSON array of tool names the inviter shared at create
-- time (`'[]'` for a standing invite's default). `max_uses`/`expires_unix`
-- are `NULL` for a standing invite (unlimited uses, never expires).
-- `revoked_unix` is set by `host.invite.revoke`; for a standing invite,
-- revoke rotates (inserts a fresh standing row) rather than leaving the
-- tenant with none. The partial unique index enforces "one LIVE standing
-- invite per inviter" -- a revoked standing row no longer counts, so
-- rotation can insert its replacement without colliding.
CREATE TABLE IF NOT EXISTS invites (
    id                INTEGER PRIMARY KEY,
    code_hash         TEXT NOT NULL UNIQUE,
    inviter_tenant_id INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    kind              TEXT NOT NULL DEFAULT 'standard' CHECK (kind IN ('standard', 'standing')),
    share_json        TEXT NOT NULL DEFAULT '[]',
    max_uses          INTEGER,
    uses              INTEGER NOT NULL DEFAULT 0,
    caller_limit      INTEGER,
    expires_unix      INTEGER,
    revoked_unix      INTEGER,
    created_unix      INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_invites_inviter ON invites(inviter_tenant_id);

CREATE UNIQUE INDEX IF NOT EXISTS idx_invites_standing_live
    ON invites(inviter_tenant_id)
    WHERE kind = 'standing' AND revoked_unix IS NULL;

-- `invite_joins`: one row per tenant created through an invite -- doubles
-- as (a) `host.invite.list`'s `invitees` join target and (b) the per-code/
-- per-inviter hourly rate-limit window (requirement 3 / 11: "20 creations
-- per code per hour", "100 per inviter per day"), both a plain windowed
-- `COUNT(*)` away, same shape as `signup_events`/`Db::try_admit_signup`.
CREATE TABLE IF NOT EXISTS invite_joins (
    id                INTEGER PRIMARY KEY,
    invite_id         INTEGER NOT NULL REFERENCES invites(id) ON DELETE CASCADE,
    inviter_tenant_id INTEGER NOT NULL,
    invitee_tenant_id INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    created_unix      INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_invite_joins_invite ON invite_joins(invite_id, created_unix);
CREATE INDEX IF NOT EXISTS idx_invite_joins_inviter ON invite_joins(inviter_tenant_id, created_unix);

-- `tenants.invited_by_tenant_id`: the inviter, for a tenant born through
-- either invite flavor; `NULL` for a tenant that signed up some other way.
-- Drives the lineage chain (`host.agent.lookup`'s `invited_by`/
-- `invitees_count`, requirement 12) and the one-hop share rule
-- (requirement 13).
ALTER TABLE tenants ADD COLUMN invited_by_tenant_id INTEGER REFERENCES tenants(id);

CREATE INDEX IF NOT EXISTS idx_tenants_invited_by ON tenants(invited_by_tenant_id);

-- `contacts.via`: how an accepted contact pair came to exist --
-- `'invite:<code>'` for one created by invite redemption, `NULL` for the
-- pre-existing manual `contact_request`/`contact_accept` flow (surfaced as
-- `"direct"` by `consent::contacts`, the same "NULL reads as the plain
-- default" convention `shared_tools`' own `via` field already uses).
ALTER TABLE contacts ADD COLUMN via TEXT;
