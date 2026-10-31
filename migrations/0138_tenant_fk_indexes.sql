-- compat: previous -- `CREATE INDEX IF NOT EXISTS` over existing columns and one wholly new table; an old release ignores an index and a table it never names (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0138_tenant_fk_indexes: PRD-mcphost-synthetic-tenant-purge R8.
--
-- Every delete of a `tenants` row looks, in each table that references it,
-- for the rows naming it. With no index whose first column is the foreign-key
-- column that lookup scans the whole table once per deleted tenant: 26
-- columns had none on prod when this was written. The synthetic purge deletes
-- thousands of tenants per sweep, so each gets one. `tests/tenp_ac03_*` lists
-- the columns through `pragma_foreign_key_list` and fails naming table and
-- column when one is uncovered, so a table added later cannot regress this.

CREATE INDEX IF NOT EXISTS idx_tenp_agent_profiles_tenant_id ON "agent_profiles"("tenant_id");
CREATE INDEX IF NOT EXISTS idx_tenp_blob_policy_tenant_id ON "blob_policy"("tenant_id");
CREATE INDEX IF NOT EXISTS idx_tenp_blocks_blocked_tenant_id ON "blocks"("blocked_tenant_id");
CREATE INDEX IF NOT EXISTS idx_tenp_channel_cursors_tenant_id ON "channel_cursors"("tenant_id");
CREATE INDEX IF NOT EXISTS idx_tenp_channel_posts_from_tenant_id ON "channel_posts"("from_tenant_id");
CREATE INDEX IF NOT EXISTS idx_tenp_channels_created_by ON "channels"("created_by");
CREATE INDEX IF NOT EXISTS idx_tenp_chart_index_tenant_id ON "chart_index"("tenant_id");
CREATE INDEX IF NOT EXISTS idx_tenp_confirm_tokens_tenant_id ON "confirm_tokens"("tenant_id");
CREATE INDEX IF NOT EXISTS idx_tenp_consent_cookies_tenant_id ON "consent_cookies"("tenant_id");
CREATE INDEX IF NOT EXISTS idx_tenp_contacts_contact_tenant_id ON "contacts"("contact_tenant_id");
CREATE INDEX IF NOT EXISTS idx_tenp_dead_letter_drops_tenant_id ON "dead_letter_drops"("tenant_id");
CREATE INDEX IF NOT EXISTS idx_tenp_doc_index_state_tenant_id ON "doc_index_state"("tenant_id");
CREATE INDEX IF NOT EXISTS idx_tenp_group_members_member_tenant_id ON "group_members"("member_tenant_id");
CREATE INDEX IF NOT EXISTS idx_tenp_invite_joins_invitee_tenant_id ON "invite_joins"("invitee_tenant_id");
CREATE INDEX IF NOT EXISTS idx_tenp_muted_muted_tenant_id ON "muted"("muted_tenant_id");
CREATE INDEX IF NOT EXISTS idx_tenp_oauth_jti_denylist_tenant_id ON "oauth_jti_denylist"("tenant_id");
CREATE INDEX IF NOT EXISTS idx_tenp_oauth_policies_tenant_id ON "oauth_policies"("tenant_id");
CREATE INDEX IF NOT EXISTS idx_tenp_orgs_owner_tenant_id ON "orgs"("owner_tenant_id");
CREATE INDEX IF NOT EXISTS idx_tenp_owner_github_states_tenant_id ON "owner_github_states"("tenant_id");
CREATE INDEX IF NOT EXISTS idx_tenp_recipe_uses_tenant_id ON "recipe_uses"("tenant_id");
CREATE INDEX IF NOT EXISTS idx_tenp_registry_documents_tenant_id ON "registry_documents"("tenant_id");
CREATE INDEX IF NOT EXISTS idx_tenp_signup_events_tenant_id ON "signup_events"("tenant_id");
CREATE INDEX IF NOT EXISTS idx_tenp_tasks_recipient_tenant_id ON "tasks"("recipient_tenant_id");
CREATE INDEX IF NOT EXISTS idx_tenp_threads_created_by ON "threads"("created_by");
CREATE INDEX IF NOT EXISTS idx_tenp_tool_caller_limits_caller_tenant_id ON "tool_caller_limits"("caller_tenant_id");
CREATE INDEX IF NOT EXISTS idx_tenp_tool_version_watermarks_caller_tenant_id ON "tool_version_watermarks"("caller_tenant_id");

-- Columns that name a tenant with no foreign key, or are only reached through
-- a join the purge counts (`src/tenant_refs.rs`): the same one-scan-per-subject
-- cost without these. Partial where most rows hold NULL.
CREATE INDEX IF NOT EXISTS idx_tenp_runs_caller_tenant_id ON "runs"("caller_tenant_id") WHERE "caller_tenant_id" IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_tenp_usage_daily_caller_tenant_id ON "usage_daily"("caller_tenant_id");
CREATE INDEX IF NOT EXISTS idx_tenp_tenants_scratch_of ON "tenants"("scratch_of") WHERE "scratch_of" IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_tenp_index_sources_tenant_id ON "index_sources"("tenant_id");
CREATE INDEX IF NOT EXISTS idx_tenp_invites_org_id ON "invites"("org_id");

-- R8: a namespace a tenant delete frees is reserved for good. `UNIQUE
-- (tenants.namespace)` alone lets a later tenant take it, and text that still
-- carries it (`runs.tool_name` on shared-call rows, `tools.forked_from`, message
-- and post `from_address`) would then resolve to a stranger. `delete_tenant_rows`
-- inserts the namespace here on every delete path; the backfill below reserves
-- the ones freed before this migration. The two actions are the only values a
-- tenant delete writes to `admin_events` (`admin.tenant_delete` and
-- `admin.tenant_delete_by_prefix` both pass `tenant_delete`; the external purge
-- passes `tenant_purge_expired`). `INSERT OR IGNORE`: idempotent at every open,
-- and `tests/tenp_ac03_*` re-runs this statement after planting rows.
CREATE TABLE IF NOT EXISTS namespace_tombstones (
    namespace   TEXT PRIMARY KEY,
    purged_unix INTEGER NOT NULL,
    action      TEXT NOT NULL
);
INSERT OR IGNORE INTO namespace_tombstones (namespace, purged_unix, action)
    SELECT tenant, COALESCE(CAST(strftime('%s', ts) AS INTEGER), 0), action
    FROM admin_events
    WHERE action IN ('tenant_delete', 'tenant_purge_expired') AND tenant IS NOT NULL
    ORDER BY id;
