-- compat: previous -- three wholly new tables (doc_chunks, doc_chunks_fts,
-- doc_index_state) an old release simply never queries; no existing
-- table's shape changes.
-- mcphost 0044_docs_index: PRD-mcphost-docs-semantic-search P0
-- requirements 1-5, P1 requirement 7.
--
-- `doc_chunks` is one row per (tenant, document, chunk_no): the paragraph
-- window's own text, its document-absolute character offset/length, an
-- optional embedding vector (little-endian f32 blob, NULL in lexical mode),
-- and `lexical_terms` (reserved for a future non-FTS5 lexical fallback;
-- unused today, FTS5 covers requirement 1's lexical index directly).
-- `doc_chunks_fts` is a standalone FTS5 table (not content-linked, so a
-- document's chunk replace is a plain delete+insert on both tables inside
-- the same transaction, no rowid-sync trigger needed) carrying the same
-- text plus enough denormalized, UNINDEXED columns (tenant_id, document_id,
-- chunk_no) to delete-by-document and to map a match back to its
-- `doc_chunks` row without a second index. `doc_index_state` is the one
-- row per tenant `docs_index.rs`'s indexer reads/writes every tick:
-- `indexed_watermark` (requirement 2's "advance only after the batch
-- commits"), the configured provider (requirement 4), and `rebuilding`/
-- `quota_chunks_reached` flags `host.docs.status` surfaces directly.
CREATE TABLE IF NOT EXISTS doc_chunks (
    tenant_id     INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    document_id   TEXT NOT NULL,
    version       INTEGER NOT NULL,
    chunk_no      INTEGER NOT NULL,
    chunk_offset  INTEGER NOT NULL,
    len           INTEGER NOT NULL,
    text          TEXT NOT NULL,
    vector        BLOB,
    lexical_terms TEXT,
    name          TEXT NOT NULL,
    created_at    INTEGER NOT NULL,
    PRIMARY KEY (tenant_id, document_id, chunk_no)
);
CREATE INDEX IF NOT EXISTS idx_doc_chunks_tenant ON doc_chunks(tenant_id);

-- requirement 3/AC2: `porter unicode61` stems both the query and the
-- indexed text ("refund" and "refunds" both stem to "refund"), so a
-- caller's own word choice doesn't have to exactly match the document's.
CREATE VIRTUAL TABLE IF NOT EXISTS doc_chunks_fts USING fts5(
    text,
    tenant_id UNINDEXED,
    document_id UNINDEXED,
    chunk_no UNINDEXED,
    tokenize = 'porter unicode61'
);

CREATE TABLE IF NOT EXISTS doc_index_state (
    tenant_id            INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    indexed_watermark    INTEGER NOT NULL DEFAULT 0,
    provider             TEXT NOT NULL DEFAULT 'none',
    endpoint             TEXT,
    model                TEXT,
    secret_name          TEXT,
    dims                 INTEGER,
    rebuilding           INTEGER NOT NULL DEFAULT 0,
    quota_chunks_reached INTEGER NOT NULL DEFAULT 0,
    updated_at           INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (tenant_id)
);
