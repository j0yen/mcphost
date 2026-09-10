-- compat: previous -- additive columns/tables; a rolled-back binary that
-- never reads them runs unmodified against the migrated schema
-- (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0012_provenance: PRD-mcphost-provenance-audit P0 requirements 1, 4.
--
-- `origin` (`'synthetic'|'external'`) is the two-way real/synthetic verdict
-- every metrics surface now reports, layered on top of the existing
-- `source_class` (`'loopback'|'fleet'|'external'`, migration 0010)
-- classification: `origin` is the coarse split `/healthz` and `admin.*`
-- surfaces expose, `source_class`/`synthetic` remain the finer-grained
-- "why" behind a synthetic verdict (see `state::derive_origin`, the one
-- function both the live write path and this migration's backfill below
-- funnel through -- Technical considerations: "one contract, many
-- callers"). `origin_detail` carries the synthorg run-id / key-class / ip
-- class that justified the verdict.
--
-- `origin` is `NOT NULL` with no *usable* default: `'unclassified'` is a
-- placeholder no live write path ever writes intentionally (every write
-- path always derives a real `'synthetic'`/`'external'` value before
-- insert -- requirement 2 / AC6, "no write path may leave origin NULL").
-- It exists only because SQLite's `ALTER TABLE ... ADD COLUMN NOT NULL`
-- requires a concrete default to backfill pre-existing rows with, and it
-- doubles as the one-shot backfill's own "not yet classified" marker --
-- the same role `source_class IS NULL` played for migration 0010's
-- backfill, just spelled as a sentinel string instead of NULL since this
-- column is never allowed to actually be NULL.
ALTER TABLE tenants ADD COLUMN origin TEXT NOT NULL DEFAULT 'unclassified';
ALTER TABLE tenants ADD COLUMN origin_detail TEXT;

-- Requirement 1/5: the durable per-signup ledger gets its own origin too
-- (independent of the tenant's current-state column, same reasoning as
-- `signup_events.synthetic` alongside `tenants.synthetic` since migration
-- 0008) plus `ip_class` (`'loopback'|'private'|'public'`, P1 requirement 5)
-- for external-unverified triage -- see `state::classify_ip_class`.
ALTER TABLE signup_events ADD COLUMN origin TEXT NOT NULL DEFAULT 'unclassified';
ALTER TABLE signup_events ADD COLUMN origin_detail TEXT;
ALTER TABLE signup_events ADD COLUMN ip_class TEXT;

-- Requirement 1: calls carry their own origin too, denormalized from the
-- calling tenant's origin at call time rather than always joined live --
-- a tenant's classification can change after the fact (retro-tagging,
-- `admin.tenant_set_synthetic`) and a call's own provenance should stay
-- exactly what it was when the call happened, not drift with it.
ALTER TABLE calls ADD COLUMN origin TEXT NOT NULL DEFAULT 'unclassified';
ALTER TABLE calls ADD COLUMN origin_detail TEXT;

-- Requirement 4: append-only admin mutation audit trail. Distinct from the
-- older `admin_events` table (migration 0005, delete-only, no actor
-- identity) -- `admin_audit` is this PRD's general-purpose, actor-tracked
-- log every admin-bearer mutation appends to centrally in
-- `handler::dispatch_admin_tool`, exposed read-only via `admin.audit_log`.
CREATE TABLE IF NOT EXISTS admin_audit (
    id           INTEGER PRIMARY KEY,
    actor_key_id TEXT NOT NULL,
    action       TEXT NOT NULL,
    target       TEXT,
    detail       TEXT,
    created_unix INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_admin_audit_created ON admin_audit(id);
