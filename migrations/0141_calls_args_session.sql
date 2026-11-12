-- compat: previous -- two additive nullable columns on an existing table and one
-- index; an old release never selects the columns and every old row reads NULL.
-- mcphost 0141_calls_args_session: PRD-mcphost-session-journal-v2 R1.
--
-- `calls.args_sha256` is the sha256 (hex) of a published-tool call's masked,
-- canonical arguments (`idempotency::args_sha256` over `Redactor` output);
-- `calls.session_id` is the MCP session id the host minted for the request.
-- Both are written once, with the row, by `Db::record_call_traced`. The index
-- serves the `session` filter of `host.calls.list` and `host.calls.sessions`.
ALTER TABLE calls ADD COLUMN args_sha256 TEXT;
ALTER TABLE calls ADD COLUMN session_id TEXT;
CREATE INDEX IF NOT EXISTS idx_calls_tenant_session ON calls(tenant_id, session_id, id);
