-- compat: previous -- one additive nullable column on the existing
-- `tenants` table plus one index; no existing column is dropped or
-- narrowed, no existing row's shape changes.
-- mcphost 0032_signup_source: PRD-mcphost-signup-kill-switch-and-source
-- requirement 2.
--
-- `signup_source`: the caller-claimed channel label from `signup`'s
-- optional `source` argument (`state::is_valid_signup_source`'s
-- `^[a-z0-9][a-z0-9._-]*$`, 1-64 chars), stored verbatim and untrusted
-- (display only -- see `control::signup`'s doc comment on keeping it
-- separate from `origin`/`origin_detail`, which this host computes
-- itself). `NULL` for every tenant that signed up without one, including
-- every tenant that existed before this migration. Indexed since
-- `/healthz`'s `signups_by_source` groups by it.
ALTER TABLE tenants ADD COLUMN signup_source TEXT;

CREATE INDEX IF NOT EXISTS idx_tenants_signup_source ON tenants(signup_source);
