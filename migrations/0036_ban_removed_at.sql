-- compat: previous -- one additive nullable column on `bans` plus its
-- index; no existing row's shape changes and no existing statement's
-- result set changes (an old release never selects `removed_at`), same
-- convention 0002/0032/0035's own compat notes follow
-- (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0036_ban_removed_at: PRD-mcphost-abuse-guard-ban-list
-- requirement 3 / AC12.
--
-- AC12 requires that a ban AND its removal both "appear in
-- `admin.ban.list` history", which a hard `DELETE FROM bans` can never
-- show. `admin.ban.remove` is therefore a soft remove: `removed_at` is the
-- removal time in unix seconds, NULL for a ban that was never removed.
-- Enforcement, the in-memory ban cache and `admin.ban.list
-- {active_only: true}` all treat a non-NULL `removed_at` as inactive,
-- exactly as they already treat a passed `expires_at`; the row stays in the
-- unfiltered listing until requirement 5's 7-day sweep collects it.
ALTER TABLE bans ADD COLUMN removed_at INTEGER;
CREATE INDEX IF NOT EXISTS idx_bans_removed_at ON bans(removed_at);
