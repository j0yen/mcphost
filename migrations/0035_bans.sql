-- compat: previous -- two wholly new tables and one additive nullable
-- column on the pre-existing `network_denials`; an old release simply
-- never queries any of them, no existing row's shape changes, no existing
-- statement's result set changes (PRD-mcphost-migration-safety requirement
-- 4, same convention 0033/0034's own compat notes already follow).
-- mcphost 0035_bans: PRD-mcphost-abuse-guard-ban-list requirement 1.
--
-- `bans`: one row per `admin.ban.add` (or auto-ban) entry, keyed by
-- `(subject_kind, subject)` -- `subject_kind` is `'key' | 'addr' |
-- 'email_domain'`; `subject` is the literal address/domain for the latter
-- two, or the sha256 hex of the raw tenant key for `'key'` (same
-- hash-at-rest convention `tenants.key_hash`/`claim_codes.code_hash`
-- already use -- a ban row never stores a credential in the clear).
-- `expires_at IS NULL` means permanent; `auto` distinguishes an
-- operator-issued ban from one `bans::tick_once` raised on its own.
CREATE TABLE IF NOT EXISTS bans (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    subject_kind TEXT NOT NULL,
    subject      TEXT NOT NULL,
    reason       TEXT NOT NULL,
    public       INTEGER NOT NULL DEFAULT 0,
    created_at   INTEGER NOT NULL,
    created_by   TEXT NOT NULL,
    expires_at   INTEGER,
    auto         INTEGER NOT NULL DEFAULT 0,
    hits         INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_bans_subject ON bans(subject_kind, subject);
CREATE INDEX IF NOT EXISTS idx_bans_expires_at ON bans(expires_at);

-- `ban_hits`: one row per refused call (same shape as `network_denials`,
-- migration 0033) -- `bans.hits` is the lifetime counter `admin.ban.list`
-- reports; this is the timestamped log `/healthz`'s windowed
-- `bans.hits_24h` needs, since a cumulative counter alone can't answer
-- "how many in the last 24h".
CREATE TABLE IF NOT EXISTS ban_hits (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    ban_id       INTEGER NOT NULL,
    created_unix INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_ban_hits_created ON ban_hits(created_unix);

-- `network_denials.tenant_id`: additive, nullable (a pre-existing row from
-- before this column, or a denial this crate can't attribute to one
-- tenant, reads as NULL and is simply excluded from the windowed count
-- below) -- requirement 4(a)'s auto-ban rule needs to know WHICH tenant's
-- denials crossed the threshold, which migration 0033's original
-- `(reason, created_unix)` shape has no way to answer.
ALTER TABLE network_denials ADD COLUMN tenant_id INTEGER;
CREATE INDEX IF NOT EXISTS idx_network_denials_tenant_created ON network_denials(tenant_id, created_unix);
