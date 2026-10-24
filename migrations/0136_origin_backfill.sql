-- compat: previous -- row UPDATEs of `tenants.origin`/`origin_detail` and
-- `signup_events.origin`/`origin_detail` only; no column added, dropped or
-- renamed, so an old release keeps reading every row.
-- mcphost 0136_origin_backfill: PRD-mcphost-external-class-positive-evidence.
--
-- `external` must be earned. The two UPDATEs on `external` rows are disjoint
-- (verified vs unverified), so statement order cannot matter. A historical
-- later-day call on an unverified `external` row is the same by-elimination
-- signal that failed on 2026-10-20 and earns nothing. `internal` and
-- `synthetic` rows are untouched. Run once (the caller records a marker in
-- `origin_counters`) and journaled with before/after counts.
UPDATE tenants SET origin = 'unattributed', origin_detail = 'legacy-external-unverified'
    WHERE origin = 'external' AND owner_verified_at IS NULL;
UPDATE tenants SET origin = 'external', origin_detail = 'claimed-email'
    WHERE origin = 'external' AND owner_verified_at IS NOT NULL;
UPDATE tenants SET origin = 'unattributed', origin_detail = 'legacy-unclassified'
    WHERE origin = 'unclassified';
UPDATE signup_events SET origin = 'unattributed', origin_detail = 'legacy-external-unverified'
    WHERE origin = 'external';
UPDATE signup_events SET origin = 'unattributed', origin_detail = 'legacy-unclassified'
    WHERE origin = 'unclassified';
