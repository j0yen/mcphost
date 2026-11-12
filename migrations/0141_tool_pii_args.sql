-- compat: previous -- one additive nullable column on `tools`, no backfill, no
-- table rebuild; an old release never selects it (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0141_tool_pii_args: PRD-mcphost-pii-tagging-and-log-redaction-v2 R1.
--
-- `pii_args_json` is the JSON array of argument names a tool's publisher declared
-- as personal data (`pii: [...]` on `host.tool.publish`, or `"x-pii": true` on a
-- schema property). NULL = untagged.
ALTER TABLE tools ADD COLUMN pii_args_json TEXT;
