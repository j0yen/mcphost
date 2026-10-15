-- compat: previous -- four wholly new objects (indexes, index_sources,
-- index_chunks, index_chunks_fts) an old release simply never queries; no
-- existing table's shape changes.
-- mcphost 0102_indexes: PRD-mcphost-index-create-from-sources R1/R3.
--
-- `indexes` is one named index per (tenant, name) with its chunking window.
-- `index_sources` is one row per source the create call named: `kind` is an
-- `IndexSourceKind` name, `ref` its prefix / table name / url, `spec_json`
-- the validated source object, `cursor` the build position (last document
-- name, last table rowid), `state` pending|done, and `items`/`chunks` the
-- counts `host.index.status` reports. `index_chunks` carries the chunks with
-- document-relative byte offsets and a NULL `vector` (lexical only);
-- `index_chunks_fts` is an external-content FTS5 table kept in step by the
-- triggers below, the way `doc_chunks_fts` is kept by its writer.
CREATE TABLE IF NOT EXISTS indexes (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    tenant_id     INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    name          TEXT NOT NULL,
    max_chars     INTEGER NOT NULL,
    overlap_chars INTEGER NOT NULL,
    created_at    INTEGER NOT NULL,
    UNIQUE (tenant_id, name)
);

CREATE TABLE IF NOT EXISTS index_sources (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    index_id   INTEGER NOT NULL REFERENCES indexes(id) ON DELETE CASCADE,
    tenant_id  INTEGER NOT NULL,
    position   INTEGER NOT NULL,
    kind       TEXT NOT NULL,
    ref        TEXT NOT NULL,
    spec_json  TEXT NOT NULL,
    state      TEXT NOT NULL DEFAULT 'pending',
    cursor     TEXT NOT NULL DEFAULT '',
    items      INTEGER NOT NULL DEFAULT 0,
    chunks     INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    updated_at INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_index_sources_pending ON index_sources(state, tenant_id);
CREATE INDEX IF NOT EXISTS idx_index_sources_index ON index_sources(index_id);

CREATE TABLE IF NOT EXISTS index_chunks (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    index_id   INTEGER NOT NULL REFERENCES indexes(id) ON DELETE CASCADE,
    source_id  INTEGER NOT NULL,
    tenant_id  INTEGER NOT NULL,
    item_ref   TEXT NOT NULL,
    chunk_no   INTEGER NOT NULL,
    byte_start INTEGER NOT NULL,
    byte_end   INTEGER NOT NULL,
    text       TEXT NOT NULL,
    vector     BLOB
);
CREATE INDEX IF NOT EXISTS idx_index_chunks_index ON index_chunks(index_id);
CREATE INDEX IF NOT EXISTS idx_index_chunks_tenant ON index_chunks(tenant_id);

CREATE VIRTUAL TABLE IF NOT EXISTS index_chunks_fts USING fts5(
    text,
    content = 'index_chunks',
    content_rowid = 'id',
    tokenize = 'porter unicode61'
);
CREATE TRIGGER IF NOT EXISTS index_chunks_ai AFTER INSERT ON index_chunks BEGIN
    INSERT INTO index_chunks_fts(rowid, text) VALUES (new.id, new.text);
END;
CREATE TRIGGER IF NOT EXISTS index_chunks_ad AFTER DELETE ON index_chunks BEGIN
    INSERT INTO index_chunks_fts(index_chunks_fts, rowid, text) VALUES ('delete', old.id, old.text);
END;
