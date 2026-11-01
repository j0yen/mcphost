-- compat: previous -- two additive nullable columns on `tools`, no backfill, no
-- table rebuild; an old release never selects them (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0138_tool_test_verdict: PRD-mcphost-trusted-discovery-catalog-receipts R1.
--
-- `test_verdict` is the verdict string the last host-run `host.tool_test` of the
-- tool returned (`pass`, `will_fail`, `unverifiable`); `test_verdict_version` is
-- the `current_version` it judged. A republish leaves both in place: the catalog
-- entry shows the verdict only while the two versions agree.
ALTER TABLE tools ADD COLUMN test_verdict TEXT;
ALTER TABLE tools ADD COLUMN test_verdict_version INTEGER;
