-- compat: previous -- one wholly new table an old release never queries; no
-- existing table's shape changes.
-- mcphost 0107_help_search_events: PRD-mcphost-docs-telemetry-failed-after-search R1.
--
-- One row per `host.help.search` call: the normalized query (never the raw
-- text), the top hit's anchor and, once the same tenant's next failed call
-- lands within 120 s, the error code and tool that followed. Anonymous calls
-- store `tenant_id NULL` and are never joined.
CREATE TABLE IF NOT EXISTS help_search_events (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    tenant_id     INTEGER,
    query_norm    TEXT NOT NULL,
    top_anchor    TEXT,
    searched_unix INTEGER NOT NULL,
    failed_code   TEXT,
    failed_tool   TEXT,
    failed_unix   INTEGER
);
CREATE INDEX IF NOT EXISTS idx_help_search_events_tenant ON help_search_events(tenant_id, searched_unix);
