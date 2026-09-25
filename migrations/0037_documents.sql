-- compat: previous -- three wholly new tables an old release simply never
-- queries; no existing table's shape changes
-- (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0037_documents: PRD-mcphost-document-store P0 requirement 1.
--
-- `documents` is one row per (tenant, document): its current version's
-- metadata (name, content hash, byte sizes, mime, caller metadata) plus
-- `seq`, a per-tenant monotonic counter bumped on every put that actually
-- changes something and every delete -- the change watermark the search
-- PRD will follow via `list {since}` rather than scanning. `document_blobs`
-- holds every kept version's raw content and extracted text as its own row
-- (`purge` drops old ones); splitting blobs out keeps `documents` rows
-- small for listing, the same "metadata table separate from bulk storage"
-- shape `tenant_state_kv`/`tenant_state_rows` already uses.
--
-- `document_usage_events` is a small named-quantity event log (requirement
-- 5: "metering emits docs.put_bytes events") -- distinct from
-- `meter_events` (migration 0007), which ledgers Stripe-bound call-count
-- batches, not a document's byte size; this table exists so `docs.rs` has
-- somewhere to record an arbitrary (event_name, quantity) fact per tenant
-- without overloading that unrelated billing ledger.
CREATE TABLE IF NOT EXISTS documents (
    id            TEXT NOT NULL,
    tenant_id     INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    name          TEXT NOT NULL,
    version       INTEGER NOT NULL,
    content_hash  TEXT NOT NULL,
    bytes         INTEGER NOT NULL,
    mime          TEXT NOT NULL,
    metadata_json TEXT NOT NULL,
    text_bytes    INTEGER NOT NULL,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL,
    deleted_at    INTEGER,
    seq           INTEGER NOT NULL,
    PRIMARY KEY (tenant_id, id)
);
CREATE INDEX IF NOT EXISTS idx_documents_tenant_name ON documents(tenant_id, name);
CREATE INDEX IF NOT EXISTS idx_documents_tenant_seq ON documents(tenant_id, seq);

CREATE TABLE IF NOT EXISTS document_blobs (
    tenant_id   INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    document_id TEXT NOT NULL,
    version     INTEGER NOT NULL,
    content     BLOB NOT NULL,
    text        TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    PRIMARY KEY (tenant_id, document_id, version)
);

CREATE TABLE IF NOT EXISTS document_usage_events (
    id           INTEGER PRIMARY KEY,
    tenant_id    INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    event_name   TEXT NOT NULL,
    quantity     INTEGER NOT NULL,
    created_unix INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_document_usage_events_tenant
    ON document_usage_events(tenant_id, event_name);
