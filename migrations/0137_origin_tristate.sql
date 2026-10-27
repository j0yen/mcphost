-- compat: previous -- one additive nullable column on `tenants`, two wholly new tables, and row UPDATEs of `origin`/`origin_detail` (values only; the columns keep their schema); an old release reads every value as an opaque string.
-- mcphost 0137_origin_tristate: PRD-mcphost-external-class-positive-evidence.
--
-- `tenants.origin_tainted_unix` is stamped (once) when an authenticated call
-- of an `unattributed` tenant came from a fleet or loopback address or
-- carried `X-Mcphost-Synthetic`. Any such call in a tenant's history blocks
-- return-promotion for good (`Db::promote_origin`, evidence `Returned`).
-- `origin` keeps `TEXT NOT NULL DEFAULT 'unclassified'`: no CHECK, no DEFAULT
-- change, no table rebuild.
ALTER TABLE tenants ADD COLUMN origin_tainted_unix INTEGER;

-- `origin_counters` holds the monotone counters of the origin contract, read by
-- `/healthz` (today only `synthetic_promotion_refused`: a claim or a qualifying
-- later-day call that arrived for a `synthetic` tenant and was refused).
CREATE TABLE IF NOT EXISTS origin_counters (
    name  TEXT PRIMARY KEY,
    value INTEGER NOT NULL DEFAULT 0
);

-- The tri-state backfill. It runs exactly once, with the batch (the Rust gate is
-- the absence of `tenants.origin_tainted_unix`). `origin_migration_journal`
-- records the per-origin counts of `tenants` and `signup_events` before and after,
-- so the two sums can be compared (nothing is created or lost, only re-labelled).
-- `calls.origin` is left as written.
CREATE TABLE IF NOT EXISTS origin_migration_journal (
    id           INTEGER PRIMARY KEY,
    phase        TEXT NOT NULL,
    table_name   TEXT NOT NULL,
    origin       TEXT NOT NULL,
    n            INTEGER NOT NULL,
    created_unix INTEGER NOT NULL
);

INSERT INTO origin_migration_journal (phase, table_name, origin, n, created_unix)
    SELECT 'before', 'tenants', origin, COUNT(*), CAST(strftime('%s', 'now') AS INTEGER)
    FROM tenants GROUP BY origin;
INSERT INTO origin_migration_journal (phase, table_name, origin, n, created_unix)
    SELECT 'before', 'signup_events', origin, COUNT(*), CAST(strftime('%s', 'now') AS INTEGER)
    FROM signup_events GROUP BY origin;

-- A verified owner is positive evidence: the detail is REWRITTEN so `claimed`
-- has one source. Every other `external` row loses the label (a later-day call
-- seen by elimination is the same signal that failed on 2026-10-20); an
-- `unclassified` row is unattributed. `synthetic` and `internal` are untouched.
UPDATE tenants SET origin = 'external', origin_detail = 'claimed-email'
    WHERE origin = 'external' AND owner_verified_at IS NOT NULL;
UPDATE tenants SET origin = 'unattributed', origin_detail = 'legacy-external-unverified'
    WHERE origin = 'external' AND owner_verified_at IS NULL;
UPDATE tenants SET origin = 'unattributed', origin_detail = 'legacy-unclassified'
    WHERE origin = 'unclassified';
UPDATE signup_events SET origin = 'unattributed', origin_detail = 'legacy-external-unverified'
    WHERE origin = 'external';
UPDATE signup_events SET origin = 'unattributed', origin_detail = 'legacy-unclassified'
    WHERE origin = 'unclassified';

INSERT INTO origin_migration_journal (phase, table_name, origin, n, created_unix)
    SELECT 'after', 'tenants', origin, COUNT(*), CAST(strftime('%s', 'now') AS INTEGER)
    FROM tenants GROUP BY origin;
INSERT INTO origin_migration_journal (phase, table_name, origin, n, created_unix)
    SELECT 'after', 'signup_events', origin, COUNT(*), CAST(strftime('%s', 'now') AS INTEGER)
    FROM signup_events GROUP BY origin;
