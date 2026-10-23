-- compat: previous -- one nullable additive column on `tenants` and one wholly
-- new table; an old release never selects either (PRD-mcphost-migration-safety
-- requirement 4). No CHECK, no DEFAULT change, no rebuild of `tenants`.
-- mcphost 0132_origin_tristate: PRD-mcphost-external-class-positive-evidence.
--
-- `tenants.origin_taint_unix` is stamped (first time only) when an
-- authenticated call of a still-`unattributed` tenant came from a fleet IP,
-- loopback, or carried `X-Mcphost-Synthetic`: such a history blocks
-- return-promotion to `external` for good. `origin_counters` holds small
-- named counters surfaced on healthz (`synthetic_promotion_refused`).
ALTER TABLE tenants ADD COLUMN origin_taint_unix INTEGER;

CREATE TABLE IF NOT EXISTS origin_counters (
    name TEXT PRIMARY KEY,
    n    INTEGER NOT NULL DEFAULT 0
);
