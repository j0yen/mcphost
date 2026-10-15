-- compat: previous -- additive nullable column on an existing table; an old
-- release simply never queries it (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0105_asks_channel: PRD-mcphost-elicitation-with-ask-fallback R5.
--
-- `asks.channel` says how the question reached the user: `link` (the signed
-- ask page, `host.ask.create` and the elicit fallback) or `client` (the
-- client's own `elicitation/create` form). NULL on rows written before this
-- column existed, which were all links.
ALTER TABLE asks ADD COLUMN channel TEXT;
