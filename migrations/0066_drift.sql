-- compat: previous -- five wholly new tables an old release simply never
-- queries; no existing table's shape changes
-- (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0066_drift: PRD-mcphost-drift-review requirement 1/3/5/6. Ported
-- from ~/projects/ai-stack @ 4ef6c22 aistack-observability's `store.rs`
-- (grounding_records/definition_versions/drift_queue/emitted_reviews) and
-- `review.rs`/`alert.rs`, adapted to mcphost's per-tenant-row schema.
--
-- `context_versions`: one row per recorded change to a table note, a
-- table's inferred schema/roles, or a document -- requirement 1.
-- `drift_queue`: one row per (tenant, kind, target, trigger_version)
-- enqueued for re-run, requirement 3's own dedupe ("enqueues once per
-- (target, version)") enforced by the unique index below. `dedupe_hash` is
-- computed at enqueue time (content-based for a version-driven trigger, a
-- fresh ULID for a manual `host.drift.check`) and carried through to
-- `emitted_reviews` at process time, so requirement 3's "one change
-- produces one review" dedupe never needs to re-derive it from
-- `context_versions`. `status` is `queued` then `done`.
-- `emitted_reviews`: requirement 3's own content-hash dedupe table -- a
-- `dedupe_hash` already present here means a review for that same change
-- content already exists, so processing the queue entry writes no second
-- `drift_reviews` row.
-- `drift_reviews`: one row per emitted `DriftReviewItem` (requirement 5).
-- `deltas_json` is the whole `QueryDelta`/`SearchDelta` array, serialized,
-- same "opaque JSON blob, real columns only for what's queried/filtered
-- on" convention `table_models.model_json` already uses. `reason` is NULL
-- while open (requirement 7's `host.drift.resolve`).
-- `drift_alerts`: requirement 6's `AlertEvent` -- one row per review whose
-- `regressed_count > 0`.
CREATE TABLE IF NOT EXISTS context_versions (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    tenant_id      INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    kind           TEXT NOT NULL,
    target         TEXT NOT NULL,
    version        INTEGER NOT NULL,
    definition_json TEXT NOT NULL,
    actor          TEXT,
    observed_unix  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_context_versions_tenant_target
    ON context_versions(tenant_id, kind, target);

CREATE TABLE IF NOT EXISTS drift_queue (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    tenant_id       INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    kind            TEXT NOT NULL,
    target          TEXT NOT NULL,
    trigger_version INTEGER NOT NULL,
    actor           TEXT,
    dedupe_hash     TEXT NOT NULL,
    queued_unix     INTEGER NOT NULL,
    status          TEXT NOT NULL DEFAULT 'queued'
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_drift_queue_dedupe
    ON drift_queue(tenant_id, kind, target, trigger_version);
CREATE INDEX IF NOT EXISTS idx_drift_queue_status ON drift_queue(status);

CREATE TABLE IF NOT EXISTS emitted_reviews (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    tenant_id     INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    dedupe_hash   TEXT NOT NULL,
    created_unix  INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_emitted_reviews_tenant_hash
    ON emitted_reviews(tenant_id, dedupe_hash);

CREATE TABLE IF NOT EXISTS drift_reviews (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    item_id        TEXT NOT NULL UNIQUE,
    tenant_id      INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    kind           TEXT NOT NULL,
    target         TEXT NOT NULL,
    old_version    INTEGER,
    new_version    INTEGER NOT NULL,
    actor          TEXT,
    created_unix   INTEGER NOT NULL,
    reason         TEXT,
    deltas_json    TEXT NOT NULL,
    changed_count  INTEGER NOT NULL,
    regressed_count INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_drift_reviews_tenant_created
    ON drift_reviews(tenant_id, created_unix);
CREATE INDEX IF NOT EXISTS idx_drift_reviews_tenant_open
    ON drift_reviews(tenant_id, reason);

CREATE TABLE IF NOT EXISTS drift_alerts (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    tenant_id       INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    review_item_id  TEXT NOT NULL,
    kind            TEXT NOT NULL,
    severity        TEXT NOT NULL,
    detail_json     TEXT NOT NULL,
    created_unix    INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_drift_alerts_tenant ON drift_alerts(tenant_id);
