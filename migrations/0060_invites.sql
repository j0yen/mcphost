-- compat: previous -- one wholly new `invites` table (CREATE TABLE IF NOT
-- EXISTS, same precedent as migrations 0011/0014/0015/0017/0019/0020/
-- 0021/0024) plus two additive nullable columns (`tenants.invited_by`,
-- `contacts.via`); no existing table's shape changes.
-- mcphost 0060_invites: PRD-mcphost-invite-links requirements 1-14.
--
-- `invites(code_hash, inviter_tenant_id, share_json, max_uses, uses,
-- expires_unix, revoked_at, caller_limit, kind, created_unix)`: one row per
-- invite URL, keyed by the SHA-256 hex of its code (same hash-at-rest
-- convention as `tenants.key_hash`/`url_secret_hash` -- `auth::hash_key`
-- over `auth::generate_invite_code`'s output, never the plaintext code).
-- `kind` is `'created'` (the capped, expiring, `host.invite.create` kind)
-- or `'standing'` (requirement 10: no expiry, no max_uses, exempt from the
-- live-invites cap -- `expires_unix`/`max_uses` are both `NULL` for this
-- kind, read as "never expires"/"unbounded" everywhere `Db::*invite*`
-- reads them). `uses` increments atomically with tenant creation
-- (`Db::try_claim_invite_use`'s own `UPDATE ... WHERE uses < max_uses`),
-- the same conditional-write admission shape `Db::try_admit_signup`
-- already uses for the per-IP signup limiter. `share_json` is a JSON
-- array of local tool names (`[]` for a standing invite's default).
-- `revoked_at` is `NULL` until `host.invite.revoke`; for a `'standing'`
-- invite, revoke rotates (requirement 11: a fresh code_hash on the SAME
-- row, `revoked_at` left `NULL`) rather than setting this column -- see
-- `Db::rotate_standing_invite`.
-- `code_plain` is `NULL` for every `'created'` invite (shown once at
-- `host.invite.create` time, never persisted in the clear -- same
-- shown-once contract the personal `/u/<secret>/mcp` URL already has) but
-- set for a `'standing'` invite, which `host.whoami` must be able to show
-- back on every call (AC9), not just the one where it was minted. A
-- standing invite is a lower-stakes credential than the personal URL
-- (requirement 10: default `share: []`; it only lets a stranger become a
-- rate-limited, lineage-tracked invitee, never acts as this tenant's own
-- bearer key), so storing it in the clear -- the same way `share_json`
-- already stores its tool-name list in the clear -- is in-bounds; the
-- non-functional "codes are never logged in clear" clause is about log
-- lines, not this column (`code_hash` keeps resolving the route exactly
-- as before; `code_plain` exists only to answer "what is my own URL").
CREATE TABLE IF NOT EXISTS invites (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    code_hash          TEXT NOT NULL UNIQUE,
    code_plain         TEXT,
    inviter_tenant_id  INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    kind               TEXT NOT NULL DEFAULT 'created',
    share_json         TEXT NOT NULL DEFAULT '[]',
    max_uses           INTEGER,
    uses               INTEGER NOT NULL DEFAULT 0,
    caller_limit       INTEGER,
    channel            TEXT,
    expires_unix       INTEGER,
    revoked_at         INTEGER,
    created_unix       INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_invites_inviter ON invites(inviter_tenant_id, kind);

-- requirement 3: invite-path admission is a per-code limiter ("at most 20
-- creations per code per hour"), not the per-IP `signup_events` limiter --
-- a separate, append-only event log keyed by `invite_id` rather than
-- `source_ip`, read the same windowed-count way `try_admit_signup` reads
-- `signup_events`.
CREATE TABLE IF NOT EXISTS invite_creation_events (
    invite_id    INTEGER NOT NULL REFERENCES invites(id) ON DELETE CASCADE,
    created_unix INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_invite_creation_events ON invite_creation_events(invite_id, created_unix);

-- requirement 12: lineage -- which invite-created tenant was invited by
-- which inviter, read by `host.agent.lookup`/`host.usage`'s `invites`
-- block. `NULL` for every tenant not created through an invite (every
-- tenant that existed before this PRD, and every `signup`/`url-page`/
-- `implicit` tenant after it).
ALTER TABLE tenants ADD COLUMN invited_by TEXT;

-- requirement 5: `host.invite.list`'s per-code `invitees: [ns]` --
-- distinct from `invited_by` above (which only names the inviter, not
-- which of their invite codes was used): the specific `invites.id` this
-- tenant was created through, `NULL` for every tenant not created via an
-- invite.
ALTER TABLE tenants ADD COLUMN invited_via_invite_id INTEGER;

-- requirement 7 / AC3: `host.agent.contacts`'s `via` field for a contact
-- pair created by an invite (`"invite:<code>"`); `NULL` for a pair created
-- through the ordinary `contact_request`/`contact_accept` dance.
ALTER TABLE contacts ADD COLUMN via TEXT;
