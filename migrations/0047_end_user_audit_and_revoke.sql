-- compat: previous -- every new table is additive, every altered table
-- (`calls`) only gains a nullable column, and the one table this PRD does
-- NOT create (`vault_tokens`, see below) is only ever given a new index --
-- so an old release simply never queries the new shapes and every existing
-- row's meaning is unchanged (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0047_end_user_audit_and_revoke: PRD-mcphost-end-user-audit-and-revoke
-- requirement 1.
--
-- `end_users` is the per-tenant roster the whole control plane (list/get/
-- audit/revoke/purge/export) reads and writes. Upserted from the batched
-- 10s flush in `enduserctl::flush_once`, never per call (non-functional:
-- "the upsert batch adds no per-call latency"); `revoked_at`/`revoked_by`/
-- `purged_at` are written synchronously by `revoke`/`unrevoke`/`purge`
-- themselves (technical considerations: "a crash loses at most 10s of
-- last-seen updates, never a revoke").
CREATE TABLE IF NOT EXISTS end_users (
    tenant_id    INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    subject      TEXT NOT NULL,
    issuer       TEXT,
    first_seen   INTEGER NOT NULL,
    last_seen    INTEGER NOT NULL,
    calls_total  INTEGER NOT NULL DEFAULT 0,
    revoked_at   INTEGER,
    revoked_by   TEXT,
    purged_at    INTEGER,
    PRIMARY KEY (tenant_id, subject)
);
-- AC10: `list`'s newest-last-seen-first keyset page over 100k rows needs
-- this composite index -- a `last_seen` scan filtered by `tenant_id` in
-- Rust would not hold the <50ms/page budget.
CREATE INDEX IF NOT EXISTS idx_end_users_tenant_last_seen
    ON end_users(tenant_id, last_seen DESC, subject DESC);

-- requirement 3 (AC3/AC7): control-plane events (revoke/unrevoke/purge
-- tombstone) scoped per tenant and per end-user subject. A new table
-- rather than widening `admin_audit` (migration 0012) -- builder choice
-- per requirement 3's "new tenant_audit table or reuse of admin_audit with
-- tenant scope -- builder chooses and cites": `admin_audit` has no
-- tenant_id/subject columns and is reserved for admin-bearer mutations,
-- written centrally from `handler::dispatch_admin_tool`; widening its
-- shape for a tenant-bearer caller it was never meant to log would blur
-- that boundary, so this control plane gets its own sibling table with the
-- same append-only shape instead.
CREATE TABLE IF NOT EXISTS tenant_audit (
    id           INTEGER PRIMARY KEY,
    tenant_id    INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    subject      TEXT NOT NULL,
    action       TEXT NOT NULL,
    detail       TEXT,
    created_unix INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_tenant_audit_tenant_subject
    ON tenant_audit(tenant_id, subject, id DESC);

-- requirement 4/5 (AC2/AC4/AC5/AC8): an end user's upstream-token
-- connections. This PRD was drafted expecting `mcphost-upstream-token-vault`
-- (visions/mcphost-end-user-auth.md component 3) to land later and extend a
-- minimal `vault_tokens` shape this control plane would own first -- but
-- that vault PRD landed first instead (migration 0046_vault.sql,
-- PRD-mcphost-upstream-token-vault), already owning `vault_tokens` with the
-- full ciphertext columns (`access_enc`/`access_nonce`/`refresh_enc`/
-- `refresh_nonce`/`expires_unix`/`scopes`) plus the same `provider`/
-- `end_user_subject`/`connected_unix`/`revoked_unix` columns this control
-- plane needs. So this migration does not repeat `CREATE TABLE ...
-- vault_tokens` -- `CREATE TABLE IF NOT EXISTS` against an already-existing
-- table is a silent no-op, so a second, narrower definition here would
-- never actually apply, and this control plane's own test-seeded rows
-- write real (dummy) values into 0046's NOT NULL ciphertext columns rather
-- than assuming a table shape that was never actually created. This
-- migration only adds the index its own subject-scoped queries
-- (`host.enduser.get`/`revoke`/`purge`) need, which 0046's own
-- `idx_vault_tokens_lookup (tenant_id, provider, end_user_subject)` doesn't
-- serve as well (no provider in hand for these lookups).
CREATE INDEX IF NOT EXISTS idx_vault_tokens_tenant_subject
    ON vault_tokens(tenant_id, end_user_subject);

-- requirement 3 (AC3): `host.enduser.audit` reports a run id per call, but
-- `calls` and `runs` rows (written together in
-- `Db::record_call_attributed_with_end_user`) were never cross-referenced.
-- Nullable and additive -- existing rows keep `NULL`, which no non-deferred
-- AC reads; every call from this release forward stamps it.
ALTER TABLE calls ADD COLUMN run_id TEXT;
