-- compat: previous -- one wholly new table; an old release simply never
-- queries it (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0132_recipe_uses: PRD-mcphost-admin-funnel-real-surface-recipe-counter P0.
--
-- One row per alias-resolved `host.quickstart` / `host.tool_publish`
-- response: which tenant, which recipe, through which verb (`via` is
-- `quickstart` or `publish`) and when. Counts-only reads join `tenants` at
-- read time (`funnel::recipe_attempts_7d`), so a later reclassification of a
-- tenant corrects the counts retroactively.
CREATE TABLE IF NOT EXISTS recipe_uses (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    tenant_id    INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    recipe       TEXT NOT NULL,
    via          TEXT NOT NULL CHECK (via IN ('quickstart', 'publish')),
    created_unix INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_recipe_uses_created ON recipe_uses(created_unix);
