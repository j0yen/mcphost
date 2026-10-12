-- compat: previous -- one wholly new table an old release simply never
-- queries; no existing table's shape changes
-- (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0085_blobs: PRD-mcphost-blob-put-get-signed-url R1.
--
-- `blobs` is one row per (tenant, blob): the metadata of its current
-- version. The bytes live on disk at `<data_dir>/blobs/<tenant_id>/<id>`,
-- not in SQLite -- a blob may be hundreds of MiB. A new put under the same
-- `name` bumps `version` on the same `id` and overwrites the file.
CREATE TABLE IF NOT EXISTS blobs (
    id         TEXT NOT NULL,
    tenant_id  INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    version    INTEGER NOT NULL,
    mime       TEXT NOT NULL,
    bytes      INTEGER NOT NULL,
    sha256     TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (tenant_id, id)
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_blobs_tenant_name ON blobs(tenant_id, name);
CREATE INDEX IF NOT EXISTS idx_blobs_id ON blobs(id);
