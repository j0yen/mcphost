-- compat: previous -- 32 `CREATE INDEX IF NOT EXISTS` and one wholly new table with a backfill; no existing
-- column or table is altered, and an old release ignores all of it (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0140_synthetic_tenant_purge: PRD-mcphost-synthetic-tenant-purge R8: index coverage for the tenant
-- delete, and the namespace tombstones.
--
-- Every `DELETE FROM tenants` scans each foreign-key column that references
-- `tenants` once; the 26 below had no index whose first column they are (the
-- tests/tenp_ac03 test enumerates them with `pragma_foreign_key_list`, so a
-- table added later cannot regress this). The five after them are columns that
-- name a tenant with no foreign key and that the purge NULLs or deletes by value
-- (`runs.caller_tenant_id` alone is read by a scan of ~700k rows on prod
-- otherwise). `IF NOT EXISTS` throughout: idempotent as-is, no table rebuild.
CREATE INDEX IF NOT EXISTS idx_agent_profiles_tenant_id ON agent_profiles(tenant_id);
CREATE INDEX IF NOT EXISTS idx_blob_policy_tenant_id ON blob_policy(tenant_id);
CREATE INDEX IF NOT EXISTS idx_blocks_blocked_tenant_id ON blocks(blocked_tenant_id);
CREATE INDEX IF NOT EXISTS idx_channel_cursors_tenant_id ON channel_cursors(tenant_id);
CREATE INDEX IF NOT EXISTS idx_channel_posts_from_tenant_id ON channel_posts(from_tenant_id);
CREATE INDEX IF NOT EXISTS idx_channels_created_by ON channels(created_by);
CREATE INDEX IF NOT EXISTS idx_chart_index_tenant_id ON chart_index(tenant_id);
CREATE INDEX IF NOT EXISTS idx_confirm_tokens_tenant_id ON confirm_tokens(tenant_id);
CREATE INDEX IF NOT EXISTS idx_consent_cookies_tenant_id ON consent_cookies(tenant_id);
CREATE INDEX IF NOT EXISTS idx_contacts_contact_tenant_id ON contacts(contact_tenant_id);
CREATE INDEX IF NOT EXISTS idx_dead_letter_drops_tenant_id ON dead_letter_drops(tenant_id);
CREATE INDEX IF NOT EXISTS idx_doc_index_state_tenant_id ON doc_index_state(tenant_id);
CREATE INDEX IF NOT EXISTS idx_group_members_member_tenant_id ON group_members(member_tenant_id);
CREATE INDEX IF NOT EXISTS idx_invite_joins_invitee_tenant_id ON invite_joins(invitee_tenant_id);
CREATE INDEX IF NOT EXISTS idx_muted_muted_tenant_id ON muted(muted_tenant_id);
CREATE INDEX IF NOT EXISTS idx_oauth_jti_denylist_tenant_id ON oauth_jti_denylist(tenant_id);
CREATE INDEX IF NOT EXISTS idx_oauth_policies_tenant_id ON oauth_policies(tenant_id);
CREATE INDEX IF NOT EXISTS idx_orgs_owner_tenant_id ON orgs(owner_tenant_id);
CREATE INDEX IF NOT EXISTS idx_owner_github_states_tenant_id ON owner_github_states(tenant_id);
CREATE INDEX IF NOT EXISTS idx_recipe_uses_tenant_id ON recipe_uses(tenant_id);
CREATE INDEX IF NOT EXISTS idx_registry_documents_tenant_id ON registry_documents(tenant_id);
CREATE INDEX IF NOT EXISTS idx_signup_events_tenant_id ON signup_events(tenant_id);
CREATE INDEX IF NOT EXISTS idx_tasks_recipient_tenant_id ON tasks(recipient_tenant_id);
CREATE INDEX IF NOT EXISTS idx_threads_created_by ON threads(created_by);
CREATE INDEX IF NOT EXISTS idx_tool_caller_limits_caller_tenant_id ON tool_caller_limits(caller_tenant_id);
CREATE INDEX IF NOT EXISTS idx_tool_version_watermarks_caller_tenant_id ON tool_version_watermarks(caller_tenant_id);

-- Columns that name a tenant without a foreign key (value match only).
CREATE INDEX IF NOT EXISTS idx_runs_caller_tenant_id ON runs(caller_tenant_id) WHERE caller_tenant_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_usage_daily_caller_tenant_id ON usage_daily(caller_tenant_id);
CREATE INDEX IF NOT EXISTS idx_tenants_scratch_of ON tenants(scratch_of) WHERE scratch_of IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_index_sources_tenant_id ON index_sources(tenant_id);
CREATE INDEX IF NOT EXISTS idx_catalog_receipts_tenant_id ON catalog_receipts(tenant_id);
-- The parent column the org cascade walks (`invites.org_id`, nullable).
CREATE INDEX IF NOT EXISTS idx_invites_org_id ON invites(org_id) WHERE org_id IS NOT NULL;

-- A purged tenant's namespace is reserved: text that still carries it
-- (`runs.tool_name` on shared-call rows, `tools.forked_from`, message and post
-- `from_address`) must never resolve to a later tenant. `DELETE FROM tenants`
-- frees the `UNIQUE` slot, so the delete writes the namespace here and the two
-- `tenants` inserts refuse a reserved one.
CREATE TABLE IF NOT EXISTS namespace_tombstones (
    namespace   TEXT PRIMARY KEY,
    purged_unix INTEGER NOT NULL,
    action      TEXT NOT NULL
);

-- Backfill: every namespace a tenant delete freed before this migration. The
-- `tenant` column of `admin_events` is the namespace; `tenant_delete` is what
-- `admin.tenant_delete` and `admin.tenant_delete_by_prefix` both write and
-- `tenant_purge_expired` what the external purge writes.
INSERT OR IGNORE INTO namespace_tombstones (namespace, purged_unix, action)
SELECT tenant, COALESCE(CAST(strftime('%s', MIN(ts)) AS INTEGER), 0), MIN(action)
  FROM admin_events
 WHERE action IN ('tenant_delete', 'tenant_purge_expired') AND tenant IS NOT NULL
 GROUP BY tenant;
