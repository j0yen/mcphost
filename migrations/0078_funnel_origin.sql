-- compat: previous -- three additive columns (one each on `tenants`,
-- `oauth_funnel_events`, `claim_email_events`) plus their indexes; an old
-- release simply never queries any of them, no existing statement's
-- result set changes (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0078_funnel_origin: PRD-mcphost-funnel-truth P0 requirement 1.
--
-- `funnel_origin` (`'human'|'fleet'|'probe'`, `state::classify_funnel_origin`'s
-- own output) is a DIFFERENT three-way split than the pre-existing
-- `tenants.origin`/`source_class` (migrations 0010/0012, `'synthetic'|
-- 'external'`/`'loopback'|'fleet'|'external'`) -- that pair answers "is
-- this safe to treat as a real paying customer" (`/healthz`'s
-- `tenants_real`, `mcphost funnel`'s real/synthetic split), this one
-- answers "whose signup is this, for the digest" (requirement 2's header +
-- fleet-list classification). A fresh column rather than repurposing
-- `origin` for the same reason migration 0032's `signup_source` picked its
-- own name instead of overloading `origin`/`origin_detail`: two unrelated
-- three-way verdicts under one column name would make every existing
-- `origin`-keyed query (dozens, across `db.rs`) silently wrong.
--
-- `NOT NULL DEFAULT 'unknown'`: same "no usable default, exists only so
-- `ALTER TABLE` can backfill old rows" posture as migration 0012's own
-- `origin` column -- every live write path (`control::signup`,
-- `invites::claim_on_first_call`, `Db::record_oauth_funnel_event`,
-- `Db::record_claim_email_failure`) always derives a real `human`/`fleet`/
-- `probe` value before insert (requirement 1/AC1: "never unknown" is a
-- live-write-path guarantee, not a backfill one -- the Goals section's
-- own "stay queryable as `unknown` where neither applies" is about
-- `mcphost admin backfill-origin`, P1 requirement 5, not this migration).
ALTER TABLE tenants ADD COLUMN funnel_origin TEXT NOT NULL DEFAULT 'unknown';
ALTER TABLE oauth_funnel_events ADD COLUMN funnel_origin TEXT NOT NULL DEFAULT 'unknown';
ALTER TABLE claim_email_events ADD COLUMN funnel_origin TEXT NOT NULL DEFAULT 'unknown';

-- requirement 4: the digest/`admin.funnel` per-origin breakdown groups by
-- `funnel_origin` over a `created_unix` window -- same composite-index
-- shape migration 0072's `idx_tenants_funnel` already gives `source_class`.
CREATE INDEX IF NOT EXISTS idx_tenants_funnel_origin ON tenants(funnel_origin, created_unix);
