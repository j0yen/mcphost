-- compat: previous -- one additive nullable column on `tenant_keys`; an old
-- release never selects it and every existing row reads as "minting session
-- unknown", which emits no `host.whoami` hint.
-- mcphost 0113_key_minted_session: PRD-mcphost-delegation-recipes-and-quickstart R6.
--
-- `minted_session` is the `Mcp-Session-Id` of the connection that called
-- `host.key.delegate`; `host.whoami` under the key from any other session adds
-- the recipe's `hint`.
ALTER TABLE tenant_keys ADD COLUMN minted_session TEXT;
