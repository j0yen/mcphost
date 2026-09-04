-- mcphost 0003: registry-publish support (PRD requirement 15 / AC19).
--
-- Adds a per-tenant "domain namespace verified" flag and the reverse-DNS
-- style namespace an admin sets it under (admin.tenant_verify_namespace)
-- via ALTER TABLE (applied conditionally in Rust, same as migration 0002,
-- since SQLite has no `ALTER TABLE ... ADD COLUMN IF NOT EXISTS`), plus a
-- table holding each tenant's most recently published server.json
-- document, served back verbatim by
-- `GET /.well-known/mcp/<namespace>/server.json`.
--
-- The domain-namespace VERIFICATION METHOD itself (DNS vs HTTP) is an open
-- question the PRD leaves to Joe; this migration only adds storage for the
-- boolean outcome an admin sets by hand.
ALTER TABLE tenants ADD COLUMN namespace_verified INTEGER NOT NULL DEFAULT 0;
ALTER TABLE tenants ADD COLUMN registry_namespace TEXT;

CREATE TABLE IF NOT EXISTS registry_documents (
    tenant_id    INTEGER PRIMARY KEY REFERENCES tenants(id),
    namespace    TEXT NOT NULL,
    document     TEXT NOT NULL,
    published_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_registry_documents_namespace ON registry_documents(namespace);
