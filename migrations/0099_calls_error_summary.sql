-- compat: previous -- additive nullable column on an existing table; an old
-- release simply never queries it (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0097_calls_error_summary: PRD-mcphost-host-doctor-tool R1.
--
-- `calls.error_summary` is `"<code>: <message first line>"`, plus the
-- tool-source line when there is one, <= 512 bytes, written only on a
-- failed call (`ok = 0`). Built from the error's message and `data.line` /
-- `data.source_line` only -- never an argument value or a secret.
ALTER TABLE calls ADD COLUMN error_summary TEXT;
