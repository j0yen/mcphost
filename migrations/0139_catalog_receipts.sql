-- compat: previous -- one new index, three wholly new tables and three triggers; no
-- existing column or table is altered, and an old release ignores all of them
-- (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0139_catalog_receipts: PRD-mcphost-trusted-discovery-catalog-receipts AC6.
--
-- The catalog's `calls_30d` receipt counts cross-tenant calls only, grouped by
-- (owner tenant, tool). Three parts keep a catalog read O(tools), not O(calls):
--
-- 1. a partial covering index holding exactly the cross-tenant rows (the
--    predicate is spelled the way `Db::search_catalog` spells it, which is what
--    lets SQLite use it), so the aggregate costs O(cross-tenant calls in the
--    window) instead of O(every call the host ever recorded);
-- 2. `catalog_receipts`, that aggregate per view (`view` 1 = every other
--    tenant counts, 0 = synthetic-marked callers left out), rebuilt by
--    `search_catalog` when `catalog_receipts_gen` moved or the build is older
--    than a minute (the 30-day window slides);
-- 3. triggers that move `catalog_receipts_gen` whenever a cross-tenant `calls`
--    row is inserted, changed or deleted, whichever statement did it.
CREATE INDEX IF NOT EXISTS idx_calls_xtenant_receipts
    ON calls(tenant_id, tool_name, started_unix, caller_tenant_id, ok)
    WHERE caller_tenant_id IS NOT NULL AND caller_tenant_id != tenant_id;

CREATE TABLE IF NOT EXISTS catalog_receipts_gen (
    id  INTEGER PRIMARY KEY CHECK (id = 1),
    gen INTEGER NOT NULL
);
INSERT OR IGNORE INTO catalog_receipts_gen (id, gen) VALUES (1, 0);

CREATE TABLE IF NOT EXISTS catalog_receipts (
    view      INTEGER NOT NULL,
    tenant_id INTEGER NOT NULL,
    tool_name TEXT    NOT NULL,
    n         INTEGER NOT NULL,
    ok        INTEGER NOT NULL,
    PRIMARY KEY (view, tenant_id, tool_name)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS catalog_receipts_built (
    view       INTEGER PRIMARY KEY,
    gen        INTEGER NOT NULL,
    built_unix INTEGER NOT NULL
);

CREATE TRIGGER IF NOT EXISTS calls_xtenant_receipts_ai AFTER INSERT ON calls
WHEN NEW.caller_tenant_id IS NOT NULL AND NEW.caller_tenant_id != NEW.tenant_id
BEGIN
    UPDATE catalog_receipts_gen SET gen = gen + 1;
END;

CREATE TRIGGER IF NOT EXISTS calls_xtenant_receipts_au
AFTER UPDATE OF tenant_id, tool_name, started_unix, ok, caller_tenant_id ON calls
WHEN (OLD.caller_tenant_id IS NOT NULL AND OLD.caller_tenant_id != OLD.tenant_id)
  OR (NEW.caller_tenant_id IS NOT NULL AND NEW.caller_tenant_id != NEW.tenant_id)
BEGIN
    UPDATE catalog_receipts_gen SET gen = gen + 1;
END;

CREATE TRIGGER IF NOT EXISTS calls_xtenant_receipts_ad AFTER DELETE ON calls
WHEN OLD.caller_tenant_id IS NOT NULL AND OLD.caller_tenant_id != OLD.tenant_id
BEGIN
    UPDATE catalog_receipts_gen SET gen = gen + 1;
END;
