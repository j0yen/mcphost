-- compat: previous -- one wholly new journal table and one wholly new taint
-- table, plus row UPDATEs of `origin`/`origin_detail` on tenants and
-- signup_events; no column or CHECK change, so a rolled-back binary that
-- never reads the new tables runs unmodified.
-- mcphost 0132_origin_tristate: PRD-mcphost-external-class-positive-evidence.
--
-- `external` is no longer the default for "not provably synthetic": it is
-- earned by positive evidence (a verified claim e-mail, or a later-UTC-day
-- return, see `Db::promote_origin`). This one-shot rewrite demotes every
-- existing `external` row that has no such evidence to `unattributed`. It is
-- gated in Rust on `origin_tristate_journal` not existing yet (a rerun
-- would demote the rows this migration and the live promoter earned).
-- The origin values below are static strings; `origin_tristate_contract`
-- asserts each is a member of `state::Origin`.
BEGIN;

CREATE TABLE origin_tristate_journal (
    id           INTEGER PRIMARY KEY,
    phase        TEXT NOT NULL,
    tbl          TEXT NOT NULL,
    origin       TEXT NOT NULL,
    n            INTEGER NOT NULL,
    created_unix INTEGER NOT NULL
);

-- Per-tenant fleet/header history the return-promotion reads: one row once a
-- tenant has made a fleet-IP or synthetic-header call; its presence blocks
-- return-promotion for good.
CREATE TABLE IF NOT EXISTS tenant_origin_taint (
    tenant_id  INTEGER PRIMARY KEY REFERENCES tenants(id) ON DELETE CASCADE,
    kind       TEXT NOT NULL,
    first_unix INTEGER NOT NULL
);

INSERT INTO origin_tristate_journal (phase, tbl, origin, n, created_unix)
    SELECT 'before', 'tenants', origin, COUNT(*), CAST(strftime('%s','now') AS INTEGER)
    FROM tenants GROUP BY origin;
INSERT INTO origin_tristate_journal (phase, tbl, origin, n, created_unix)
    SELECT 'before', 'signup_events', origin, COUNT(*), CAST(strftime('%s','now') AS INTEGER)
    FROM signup_events GROUP BY origin;

-- A verified claim e-mail is the positive evidence; the detail is rewritten
-- from `external-unverified` so `claimed` has one source.
UPDATE tenants SET
    origin = CASE WHEN owner_verified_at IS NOT NULL THEN 'external' ELSE 'unattributed' END,
    origin_detail = CASE WHEN owner_verified_at IS NOT NULL THEN 'claimed-email'
                         ELSE 'legacy-external-unverified' END
    WHERE origin = 'external';
UPDATE tenants SET origin = 'unattributed', origin_detail = 'legacy-unclassified'
    WHERE origin = 'unclassified';

-- A signup event records signup-time evidence, and there was none.
UPDATE signup_events SET origin = 'unattributed', origin_detail = 'legacy-external-unverified'
    WHERE origin = 'external';
UPDATE signup_events SET origin = 'unattributed', origin_detail = 'legacy-unclassified'
    WHERE origin = 'unclassified';

INSERT INTO origin_tristate_journal (phase, tbl, origin, n, created_unix)
    SELECT 'after', 'tenants', origin, COUNT(*), CAST(strftime('%s','now') AS INTEGER)
    FROM tenants GROUP BY origin;
INSERT INTO origin_tristate_journal (phase, tbl, origin, n, created_unix)
    SELECT 'after', 'signup_events', origin, COUNT(*), CAST(strftime('%s','now') AS INTEGER)
    FROM signup_events GROUP BY origin;

COMMIT;
