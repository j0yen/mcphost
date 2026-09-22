-- compat: previous -- three additive nullable columns on the existing
-- `channels` table (migration 0027) plus one partial-unique index; no
-- existing table's shape changes, no existing column is dropped or
-- narrowed.
-- mcphost 0029_agent_channels: PRD-mcphost-agent-channels P0 requirements
-- 1, 2, 6, 8; P1 requirement 10.
--
-- Migration 0027 (PRD-mcphost-agent-mesh-ops) built a minimal, ungated
-- `host.channel.open(name)`/`host.channel.post(channel)` vertical slice
-- against a global, name-keyed `channels` table with no membership concept
-- -- the only `host.channel.*` surface that existed when this PRD started.
-- This PRD is the group-based channel this PRD's own PRD text describes;
-- rather than a second, differently-named `channels`-like table (which
-- would fork `channel_posts`/`channel_cursors` too, and split
-- `admin.mesh.*`'s existing counters/purge across two physical stores),
-- it extends the same table: `group_id` ties a channel to the group it was
-- opened for (`host.channel.open(group=...)`), NULL for a legacy
-- name-only channel and never both -- `idx_channels_group_id` enforces
-- "one channel per group" (requirement 2) the same way migration 0027's
-- own `UNIQUE(name)` already enforces "one channel per name" for the
-- legacy slice. `closed_at`/`frozen_at` are this PRD's own requirement 2
-- (close) and P1 requirement 10 (freeze/unfreeze); both NULL means open
-- and not frozen, the migration-time default for every existing row
-- (every legacy channel included, since neither ever applied to it).
--
-- `channel_posts`/`channel_cursors` (already migration 0027) are reused
-- as-is for a group channel's posts/cursors -- ordinary rows keyed by the
-- same `channel_id`, no new column needed: `channel_posts.seq` is already
-- dense per channel (`UNIQUE(channel_id, seq)`), and `channel_cursors.seq`
-- already stores exactly "the last seq this tenant has acknowledged",
-- this PRD's own `last_seq`.
ALTER TABLE channels ADD COLUMN group_id INTEGER REFERENCES groups(id) ON DELETE CASCADE;
ALTER TABLE channels ADD COLUMN closed_at TEXT;
ALTER TABLE channels ADD COLUMN frozen_at TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS idx_channels_group_id ON channels(group_id) WHERE group_id IS NOT NULL;
