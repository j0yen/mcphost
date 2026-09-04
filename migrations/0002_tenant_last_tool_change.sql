-- mcphost 0002: track the last tool-set change per tenant (a publish OR a
-- remove) so tools/list's ttlMs cache hint (PRD requirement 14 / AC18)
-- stays honest after a remove deletes the very tools row that would
-- otherwise carry the timestamp. Applied conditionally in Rust (see
-- Db::migrate_0002_tenant_last_tool_change) since SQLite has no
-- `ALTER TABLE ... ADD COLUMN IF NOT EXISTS`.
ALTER TABLE tenants ADD COLUMN last_tool_change_unix INTEGER NOT NULL DEFAULT 0;
