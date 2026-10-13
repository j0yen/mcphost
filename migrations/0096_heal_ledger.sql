-- compat: previous -- additive nullable/defaulted columns on an existing
-- table; an old release simply never queries them
-- (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0095_heal_ledger: PRD-mcphost-heal-action-registry-and-ledger R2.
--
-- An `incidents` row doubles as the self-heal ledger: `action` names the
-- `HealAction` (NULL on every pre-existing, prose-only row), `outcome` is a
-- `HealOutcome` (ok, failed, refused, would_run, rate_limited), and
-- `trigger_json`/`before_json`/`after_json` carry the evidence.
ALTER TABLE incidents ADD COLUMN action TEXT;
ALTER TABLE incidents ADD COLUMN trigger_json TEXT;
ALTER TABLE incidents ADD COLUMN before_json TEXT;
ALTER TABLE incidents ADD COLUMN after_json TEXT;
ALTER TABLE incidents ADD COLUMN outcome TEXT;
ALTER TABLE incidents ADD COLUMN dry_run INTEGER NOT NULL DEFAULT 0;
ALTER TABLE incidents ADD COLUMN duration_ms INTEGER;
