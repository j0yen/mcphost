-- compat: previous -- two additive nullable columns on the existing
-- `tenants` table plus one unique index; no existing column is dropped or
-- narrowed, no existing row's shape changes.
-- mcphost 0059_url_bound_tenants: PRD-mcphost-url-bound-tenants
-- requirement 2.
--
-- `url_secret_hash`: SHA-256 hex of this tenant's secret-URL token
-- (`auth::generate_url_secret`, stored via `auth::hash_key` -- the same
-- hashing convention as `key_hash`), never the plaintext secret itself.
-- `NULL` until this tenant's URL secret is first generated (today: the
-- `/u/new` signup page, or `host.key_rotate`). A unique index means a
-- collision in `auth::generate_url_secret`'s 128 random bits would surface
-- as a write failure rather than silently handing two tenants the same
-- URL; SQLite treats every `NULL` as distinct from every other `NULL`, so
-- the (overwhelmingly common, pre-URL) unset rows never conflict with each
-- other.
--
-- `url_rotated_at`: unix seconds of the last time that secret changed
-- (set or rotated alike), `NULL` until then.
ALTER TABLE tenants ADD COLUMN url_secret_hash TEXT;
ALTER TABLE tenants ADD COLUMN url_rotated_at INTEGER;

CREATE UNIQUE INDEX IF NOT EXISTS idx_tenants_url_secret_hash ON tenants(url_secret_hash);
