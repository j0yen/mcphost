-- compat: previous -- one nullable additive column on `logs`; an old release
-- never selects it, and every existing line reads as written by the primary.
-- mcphost 0129_logs_key_id: PRD-mcphost-acting-for-in-ledger-and-audit R5.
--
-- `key_id` is the `tenant_keys.id` of the delegated key whose call wrote the
-- line, so `host.tool.logs` can render an `[acting_for=...]` prefix without
-- ever changing the stored `line`.
ALTER TABLE logs ADD COLUMN key_id INTEGER;
