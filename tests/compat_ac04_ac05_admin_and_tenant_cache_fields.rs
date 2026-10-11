//! AC4 — Given a client authenticated with the admin key, When it calls
//! `tools/list`, Then the result contains a numeric `ttlMs` and
//! `cacheScope: "private"` and lists the `admin.*` tools.
//! AC5 — Given a tenant that has published no tools, When it calls
//! `tools/list`, Then the result contains a numeric `ttlMs` and
//! `cacheScope: "private"` and lists the ten `host.*` tools plus
//! `host.tool_call` (PRD-mcphost-session-key requirement 11).
//!
//! (AC6 and AC7 -- the tenant's `ttlMs` going to `0` within the grace
//! window after a publish/remove, and back to the steady TTL once the
//! window elapses -- are already covered end to end, unmodified, by
//! `tests/ac18_tools_list_ttl.rs`; this PRD's requirement 4 is that that
//! behaviour is preserved exactly, not re-tested here.)

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, signup};

#[tokio::test]
async fn ac4_admin_tools_list_has_cache_fields() {
    let server = TestServer::start().await;
    let client = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    let tools = client
        .tools_list()
        .await
        .expect("tools/list should succeed for the admin key");

    assert!(
        tools["ttlMs"].is_u64(),
        "ttlMs must be a JSON number: {tools}"
    );
    assert_eq!(
        tools["cacheScope"].as_str(),
        Some("private"),
        "cacheScope must be \"private\": {tools}"
    );
    let names: Vec<&str> = tools["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(
        !names.is_empty() && names.iter().all(|n| n.starts_with("admin.")),
        "admin tools/list must list only admin.* tools: {names:?}"
    );
}

#[tokio::test]
async fn ac5_fresh_tenant_tools_list_has_cache_fields() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Fresh Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let tools = client
        .tools_list()
        .await
        .expect("tools/list should succeed for a tenant with no published tools");

    assert!(
        tools["ttlMs"].is_u64(),
        "ttlMs must be a JSON number: {tools}"
    );
    assert_eq!(
        tools["cacheScope"].as_str(),
        Some("private"),
        "cacheScope must be \"private\": {tools}"
    );
    let names: Vec<&str> = tools["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(
        names
            .iter()
            .all(|n| n.starts_with("host.") || n.starts_with("billing.") || n.starts_with("host_") || n.starts_with("billing_")),
        "a tenant with no published tools must see only host.*/billing.* tools: {names:?}"
    );
    // PRD-mcphost-tool-naming-convention-and-aliases requirement 2/3: 147
    // -> 167 (+20) -- every host.tool_*-family/host.key_rotate/
    // host.self_offboard/host.secret_*/host.registry_publish/
    // host.bridge_test/host.spec_test violator is now registered under
    // its dotted canonical name AND kept working under its old name as a
    // deprecated alias, so the control-plane surface this tenant sees
    // gained 20 names (the aliases) with none removed.
    assert_eq!(
        names.len(),
        // PRD-mcphost-tool-naming-convention-and-aliases rebase onto main
        // (which had grown 147 -> 150 with host.table.graph/join_paths/
        // next_questions, then 150 -> 151 with host.table.query_log from
        // PRD-mcphost-table-context-and-sql-passthrough, then 151 -> 155
        // with host.drift.reviews/review/resolve/check from
        // PRD-mcphost-drift-review):
        // 155 + 20 aliases = 175, then 175 -> 178 with host.invite.create/
        // list/revoke from PRD-mcphost-invite-links, then 178 -> 180 with
        // host.table.query_diagnose/query_stats (PRD-mcphost-query-diagnosis),
        // then r368-prep rebase (2026-10-03) 180 -> 183 with this PRD's own
        // three new host.table.handles/handle_drop/handle_export tools.
        // PRD-mcphost-row-policy rebase onto main (run 353, 2026-10-04):
        // this PRD's own five host.policy.set/list/attrs_set and
        // host.audit.chain/verify tools join on top -- 183 + 5 = 188.
        // PRD-mcphost-tools-list-alias-truth: +168 flattened `a_b_c` forms
        // (one per dotted canonical), 188 + 168 = 356.
        // PRD-mcphost-uptime-probe-recipe-green: host.uptime.create, one
        // new dotted tool plus its flattened form, 356 + 2 = 358.
        358,
        "there must be exactly the sixteen host.* control-plane tools \
         (incl. host.quickstart, host.tool_run, host.bridge_test, host.spec_test -- \
         PRD-mcphost-tool-test, and host.redeem/host.key_rotate -- \
         PRD-mcphost-handoff-token) plus host.tool_call plus the three \
         host.tool_history/host.tool_rollback/host.tool_diff tools \
         (PRD-mcphost-tool-versions) plus the nine host.state.* tools \
         (PRD-mcphost-tenant-state) plus the eighteen host.table.* tools \
         (PRD-mcphost-tenant-tables, PRD-mcphost-table-semantic-model, \
         PRD-mcphost-chart-in-a-minute, PRD-mcphost-table-context-and-sql-passthrough, \
         PRD-mcphost-result-handles) plus the nine \
         host.tool_share/host.tool_spec_shared/host.tool_unshare/ \
         host.group.*/host.catalog.* tools (PRD-mcphost-sharing, \
         PRD-mcphost-shared-tool-spec-readback) plus the two \
         host.share.caller_limit/caller_limit_remove tools \
         (PRD-mcphost-shared-tool-caller-usage) plus the five \
         host.runs.* tools (PRD-mcphost-runs-and-jobs) plus the two \
         host.progress/host.runs.part tools (PRD-mcphost-run-result-overflow-to-state) \
         plus the nine host.trigger.* \
         tools (PRD-mcphost-schedules, PRD-mcphost-inbound-events) plus the three billing.* \
         tools plus the four host.agent.* tools (PRD-mcphost-agent-directory) plus the \
         eight host.msg.* tools (PRD-mcphost-agent-inbox, PRD-mcphost-agent-wake) plus the \
         seven host.agent.contact_*/mute/unmute tools (PRD-mcphost-agent-consent) plus \
         host.self_offboard (PRD-mcphost-tenant-self-offboard) + host.changelog (PRD-mcphost-host-tool-deprecation) plus \
         host.export (PRD-mcphost-tenant-data-export) plus \
         the six host.channel.* tools (PRD-mcphost-agent-mesh-ops, PRD-mcphost-agent-channels) + the six host.docs.* tools (PRD-mcphost-document-store) plus \
         the five host.oauth.* tools (PRD-mcphost-oauth-resource-server, PRD-mcphost-hosted-authorization-server) plus \
         the four host.oauth.provider_set/provider/provider_remove/doctor tools (PRD-mcphost-federated-end-user-login) plus \
         the three host.oauth.trusted_issuer_set/trusted_issuer_remove/trusted_issuers tools (PRD-mcphost-enterprise-managed-auth) plus \
         the three host.docs.search/index_config/reindex tools (PRD-mcphost-docs-semantic-search) plus \
         the two host.enduser.* tools (PRD-mcphost-end-user-identity) plus \
         the eight host.oauth.policy_set/policy/pending/client_approve/client_deny/revoke_all/ \
         audit/audit_export tools (PRD-mcphost-oauth-client-policy) plus \
         the three host.lineage.* tools (PRD-mcphost-lineage-blast-radius) plus \
         the six host.vault.* tools (PRD-mcphost-upstream-token-vault, \
         PRD-mcphost-upstream-token-vault-status) plus \
         the three host.table.graph/join_paths/next_questions tools \
         (PRD-mcphost-table-concept-graph) plus \
         the four host.drift.reviews/review/resolve/check tools \
         (PRD-mcphost-drift-review) plus the 20 deprecated-alias tools \
         (PRD-mcphost-tool-naming-convention-and-aliases) plus \
         host.table.query_diagnose/query_stats (PRD-mcphost-query-diagnosis) plus \
         the five host.policy.*/host.audit.chain/host.audit.verify tools (PRD-mcphost-row-policy): {names:?}"
    );
}
