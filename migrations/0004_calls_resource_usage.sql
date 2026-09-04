-- compat: previous -- additive ALTER TABLE ADD COLUMN (nullable); the previous
-- release's calls queries name their columns explicitly and are unaffected.
-- mcphost 0004: PRD-mcphost-code-tools requirement 8 -- the `python` kind's
-- sandboxed calls meter CPU time and peak resident memory; every kind's
-- `calls` row gains these two nullable columns via the same conditional
-- ALTER TABLE pattern as migrations 0002/0003 (SQLite has no `ADD COLUMN IF
-- NOT EXISTS`). NULL for calls to kinds (`echo`, `http`) that don't meter a
-- subprocess.
ALTER TABLE calls ADD COLUMN cpu_ms INTEGER;
ALTER TABLE calls ADD COLUMN peak_rss_kb INTEGER;
