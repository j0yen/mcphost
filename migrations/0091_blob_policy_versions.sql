-- compat: previous -- two wholly new tables an old release simply never
-- queries; no existing table's shape changes
-- (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0087_blob_policy_versions: PRD-mcphost-blob-retention-and-quota R2/R6.
--
-- `blob_policy` is one lazily-created row per tenant: the defaults a put
-- falls back to (`default_ttl_s`, NULL = never), how many versions of one
-- name stay live (`keep_latest_versions`, NULL = only the current one, as
-- before), and how long before expiry the hourly tick tells the inbox
-- (`notify_before_s`, 0 = off). `last_notice_unix` is when that notice last
-- went out (one per tenant per day).
CREATE TABLE IF NOT EXISTS blob_policy (
    tenant_id            INTEGER PRIMARY KEY REFERENCES tenants(id) ON DELETE CASCADE,
    default_ttl_s        INTEGER,
    keep_latest_versions INTEGER,
    notify_before_s      INTEGER NOT NULL DEFAULT 0,
    last_notice_unix     INTEGER NOT NULL DEFAULT 0
);
-- `blob_versions` holds the superseded versions of a name while a tenant's
-- policy keeps more than one. Their bytes live beside the current file at
-- `<data_dir>/blobs/<tenant_id>/<id>.v<version>`. `expired` marks a version
-- the policy has aged out; `expires_at` is then the moment it was marked.
CREATE TABLE IF NOT EXISTS blob_versions (
    tenant_id  INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    id         TEXT NOT NULL,
    version    INTEGER NOT NULL,
    name       TEXT NOT NULL,
    mime       TEXT NOT NULL,
    bytes      INTEGER NOT NULL,
    sha256     TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER,
    expired    INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (tenant_id, id, version)
);
