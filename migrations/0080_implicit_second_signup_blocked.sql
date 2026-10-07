-- compat: previous -- one additive column on `signup_events`; an old
-- release simply never queries `kind`, no existing statement's result set
-- changes (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0080_implicit_second_signup_blocked: PRD-mcphost-session-bound-tenant-key
-- requirement 3 / AC2. Renumbered to 0080 during this rebase: mcphost-tool-call-
-- host-verb-forward claimed 0079 first, landing on main ahead of this branch
-- (this PRD's own migration was originally numbered 0079).
--
-- `NOT NULL DEFAULT 'signup'`: every row this table has ever held (the
-- real-signup rate-limit ledger `Db::try_admit_signup`/`record_signup_event*`
-- already write) backfills as `kind = 'signup'` -- the default IS the
-- pre-existing meaning, so `Db::signup_count_since`/`Db::try_admit_signup`'s
-- own `WHERE kind = 'signup'` addition (this PRD's own change, not this
-- migration) keeps counting every row it always did and nothing else. A
-- second value, `implicit_second_signup_blocked`, is written only by
-- `Db::record_blocked_implicit_second_signup` -- a refused call that never
-- created a tenant, so it must never count toward that same rate limit.
ALTER TABLE signup_events ADD COLUMN kind TEXT NOT NULL DEFAULT 'signup';

-- requirement 3: `admin.funnel`'s own per-day `implicit_second_signup_blocked`
-- count groups by `kind` over a `created_unix` window, same composite-index
-- shape migration 0078's `idx_tenants_funnel_origin` already gives its own
-- per-day breakdown.
CREATE INDEX IF NOT EXISTS idx_signup_events_kind_time ON signup_events(kind, created_unix);
