-- compat: previous -- three wholly new nullable/defaulted columns an old
-- release simply never queries; no existing column's shape changes.
-- mcphost 0056_tool_spec_exposure: PRD-mcphost-shared-tool-spec-readback
-- P0 requirements 1/2/5, P1 requirement 9. (Renumbered from this PRD's own
-- 0045 during the merge of origin/main, which had already claimed 0045
-- through 0055 for other PRDs.)
--
-- `expose_spec` is a per-share opt-in (default off, requirement 1): when
-- true, any tenant the tool is shared with may call `host.tool_spec_shared`
-- to read a redacted `spec`. `spec_exposed_unix` is the unix-seconds
-- timestamp of the share call that turned exposure on -- `NULL` whenever
-- `expose_spec` is 0, cleared alongside it by `host.tool_unshare`/
-- `admin.tool_unshare` and by a re-share that omits the flag (requirement
-- 5). `spec_reads` counts successful `host.tool_spec_shared` reads since the
-- most recent `host.tool_share` call (requirement 9/AC9), reset to 0 by the
-- same events that clear `spec_exposed_unix`.
ALTER TABLE tools ADD COLUMN expose_spec INTEGER NOT NULL DEFAULT 0;
ALTER TABLE tools ADD COLUMN spec_exposed_unix INTEGER;
ALTER TABLE tools ADD COLUMN spec_reads INTEGER NOT NULL DEFAULT 0;
