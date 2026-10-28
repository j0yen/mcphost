-- compat: previous -- one wholly new table and `CREATE INDEX IF NOT EXISTS` over existing columns; an old release never selects the table and ignores the indexes.
-- mcphost 0138_synthetic_tenant_purge: PRD-mcphost-synthetic-tenant-purge R8.
--
-- (1) Every foreign-key column referencing `tenants` gets an index whose first
-- column it is, so a tenant delete's cascade does an index lookup per child
-- table instead of a scan (26 columns had none on prod). A table added later
-- is held to the same rule by tests/tenp_ac03_*.
-- (2) `namespace_tombstones` reserves every namespace a tenant delete freed, so
-- text that still carries it (`runs.tool_name` on shared-call rows,
-- `tools.forked_from`, message and post `from_address`) can never resolve to a
-- later tenant. Backfilled from the `admin_events` rows a tenant delete wrote
-- (`tenant_delete`, `tenant_purge_expired`; the `tenant` column is the namespace).
CREATE INDEX IF NOT EXISTS idx_signup_events_tenant_id ON signup_events(tenant_id);
CREATE INDEX IF NOT EXISTS idx_registry_documents_tenant_id ON registry_documents(tenant_id);
CREATE INDEX IF NOT EXISTS idx_group_members_member_tenant_id ON group_members(member_tenant_id);
CREATE INDEX IF NOT EXISTS idx_agent_profiles_tenant_id ON agent_profiles(tenant_id);
CREATE INDEX IF NOT EXISTS idx_threads_created_by ON threads(created_by);
CREATE INDEX IF NOT EXISTS idx_blocks_blocked_tenant_id ON blocks(blocked_tenant_id);
CREATE INDEX IF NOT EXISTS idx_contacts_contact_tenant_id ON contacts(contact_tenant_id);
CREATE INDEX IF NOT EXISTS idx_muted_muted_tenant_id ON muted(muted_tenant_id);
CREATE INDEX IF NOT EXISTS idx_channels_created_by ON channels(created_by);
CREATE INDEX IF NOT EXISTS idx_channel_posts_from_tenant_id ON channel_posts(from_tenant_id);
CREATE INDEX IF NOT EXISTS idx_channel_cursors_tenant_id ON channel_cursors(tenant_id);
CREATE INDEX IF NOT EXISTS idx_tool_version_watermarks_caller_tenant_id ON tool_version_watermarks(caller_tenant_id);
CREATE INDEX IF NOT EXISTS idx_tool_caller_limits_caller_tenant_id ON tool_caller_limits(caller_tenant_id);
CREATE INDEX IF NOT EXISTS idx_doc_index_state_tenant_id ON doc_index_state(tenant_id);
CREATE INDEX IF NOT EXISTS idx_oauth_jti_denylist_tenant_id ON oauth_jti_denylist(tenant_id);
CREATE INDEX IF NOT EXISTS idx_oauth_policies_tenant_id ON oauth_policies(tenant_id);
CREATE INDEX IF NOT EXISTS idx_chart_index_tenant_id ON chart_index(tenant_id);
CREATE INDEX IF NOT EXISTS idx_invite_joins_invitee_tenant_id ON invite_joins(invitee_tenant_id);
CREATE INDEX IF NOT EXISTS idx_dead_letter_drops_tenant_id ON dead_letter_drops(tenant_id);
CREATE INDEX IF NOT EXISTS idx_blob_policy_tenant_id ON blob_policy(tenant_id);
CREATE INDEX IF NOT EXISTS idx_confirm_tokens_tenant_id ON confirm_tokens(tenant_id);
CREATE INDEX IF NOT EXISTS idx_tasks_recipient_tenant_id ON tasks(recipient_tenant_id);
CREATE INDEX IF NOT EXISTS idx_orgs_owner_tenant_id ON orgs(owner_tenant_id);
CREATE INDEX IF NOT EXISTS idx_consent_cookies_tenant_id ON consent_cookies(tenant_id);
CREATE INDEX IF NOT EXISTS idx_owner_github_states_tenant_id ON owner_github_states(tenant_id);
CREATE INDEX IF NOT EXISTS idx_recipe_uses_tenant_id ON recipe_uses(tenant_id);

-- The columns that name a tenant with no foreign key, or that a tenant delete
-- looks up by value (the purge stage NULLs or deletes them per subject).
CREATE INDEX IF NOT EXISTS idx_runs_caller_tenant_id ON runs(caller_tenant_id) WHERE caller_tenant_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_usage_daily_caller_tenant_id ON usage_daily(caller_tenant_id);
CREATE INDEX IF NOT EXISTS idx_invites_org_id ON invites(org_id) WHERE org_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_tenants_scratch_of ON tenants(scratch_of) WHERE scratch_of IS NOT NULL;

CREATE TABLE IF NOT EXISTS namespace_tombstones (
    namespace   TEXT PRIMARY KEY,
    purged_unix INTEGER NOT NULL,
    action      TEXT NOT NULL
);

INSERT OR IGNORE INTO namespace_tombstones (namespace, purged_unix, action)
SELECT tenant, COALESCE(CAST(strftime('%s', ts) AS INTEGER), 0), action FROM admin_events
 WHERE action IN ('tenant_delete', 'tenant_purge_expired') AND tenant IS NOT NULL AND tenant <> '';
