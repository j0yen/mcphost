-- compat: previous -- additive nullable / defaulted columns and one new table; an old
-- release never reads them (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0106_index_refresh: PRD-mcphost-index-refresh-on-change R1/R2.
--
-- `index_sources.marker` is the change marker the last pass targeted and
-- `marker_seen` the one the index content reflects (NULL = a refresh was
-- forced); `refresh_s` is a url source's interval; `last_checked_at` /
-- `last_changed_at` time the last look and the last change; `fail_count` /
-- `paused` are the three-strikes url breaker. `index_chunks.text_hash` is
-- sha256(text) so an identical re-chunk keeps its vector.
-- `table_change_counters` is the per-table write counter, in the main db so
-- the index tick reads it without opening the tenant db: `change_counter`
-- moves on every write, `rewrite_counter` on writes that are not appends.
CREATE TABLE IF NOT EXISTS table_change_counters (
    tenant_id       INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    table_name      TEXT NOT NULL,
    change_counter  INTEGER NOT NULL DEFAULT 0,
    rewrite_counter INTEGER NOT NULL DEFAULT 0,
    changed_at      INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (tenant_id, table_name)
);
CREATE INDEX IF NOT EXISTS idx_index_chunks_item ON index_chunks(source_id, item_ref);
ALTER TABLE index_chunks ADD COLUMN text_hash TEXT;
ALTER TABLE index_sources ADD COLUMN fail_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE index_sources ADD COLUMN paused INTEGER NOT NULL DEFAULT 0;
ALTER TABLE index_sources ADD COLUMN refresh_s INTEGER;
ALTER TABLE index_sources ADD COLUMN last_checked_at INTEGER;
ALTER TABLE index_sources ADD COLUMN last_changed_at INTEGER;
ALTER TABLE index_sources ADD COLUMN marker_seen TEXT;
ALTER TABLE index_sources ADD COLUMN marker TEXT;
