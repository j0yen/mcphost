-- compat: previous -- one wholly new table (agent_profiles, CREATE TABLE IF
-- NOT EXISTS, same precedent as migrations 0011/0014/0015/0019) plus one
-- additive `tenants.last_seen_unix` column an old release simply never
-- queries; no existing table's shape changes.
-- mcphost 0020_agent_profiles: PRD-mcphost-agent-directory requirements 1/6.
--
-- `agent_profiles` is the optional identity card every tenant reads as even
-- without a row (requirement 1: defaults applied at read in Rust -- see
-- `Db::agent_profile`/`Db::lookup_agent`). `handle` is the only field a
-- tenant must explicitly claim (`host.agent.profile_set`); SQLite's UNIQUE
-- constraint treats NULL as distinct from every other NULL, so any number
-- of unclaimed tenants can share `handle IS NULL` without colliding.
--
-- `tenants.last_seen_unix` (requirement 6): the tenant's most recent
-- authenticated request, bumped from `handler::resolve_auth`/
-- `resolve_tenant_key_auth` on every call that resolves to a live tenant.
-- `host.agent.lookup`/`host.agent.search` round it to the minute at read
-- time (never stored rounded, so nothing downstream loses precision it
-- might one day want).
CREATE TABLE IF NOT EXISTS agent_profiles (
    tenant_id      INTEGER PRIMARY KEY REFERENCES tenants(id) ON DELETE CASCADE,
    handle         TEXT UNIQUE,
    description    TEXT,
    tags_json      TEXT NOT NULL DEFAULT '[]',
    contact_policy TEXT NOT NULL DEFAULT 'open',
    updated_at     TEXT
);

CREATE INDEX IF NOT EXISTS idx_agent_profiles_handle ON agent_profiles(handle);

ALTER TABLE tenants ADD COLUMN last_seen_unix INTEGER;
