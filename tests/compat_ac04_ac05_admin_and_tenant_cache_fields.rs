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
use mcphost::kinds::KindRegistry;

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
    // The expected host-tool count is DERIVED, never typed: the registry
    // that builds `tools/list` is the one component that knows the set
    // (`host_tool_descriptors` = `host_tools(kinds, authenticated: true)`,
    // canonicals + deprecated aliases + flattened forms). A PRD that adds a
    // tool changes this number by changing the registry, not this file;
    // `committed_contract_names_equal_the_registry_names` below pins the
    // registry to the committed contract so the derivation cannot drift.
    let expected = mcphost::handler::host_tool_descriptors(&KindRegistry::with_builtin()).len();
    assert_eq!(
        names.len(),
        expected,
        "a tenant with no published tools must see exactly the registry's host.*/billing.* \
         control-plane tools ({expected}, derived from KindRegistry::with_builtin() + \
         host_tool_descriptors): {names:?}"
    );
}

/// Drift guard for the derived count above: the registry's host-tool name set
/// must equal the name set in the committed `contracts/host-tools.v1.json`.
/// A disagreement names the extra / missing tools. Fix by running
/// `mcphost contract dump` (never by hand-editing the file or this test).
#[test]
fn committed_contract_names_equal_the_registry_names() {
    use std::collections::BTreeSet;

    let registry: BTreeSet<String> = mcphost::handler::host_tool_descriptors(&KindRegistry::with_builtin())
        .iter()
        .map(|t| t.name.to_string())
        .collect();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(mcphost::api_contract::DEFAULT_CONTRACT_PATH);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let contract: serde_json::Value =
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{} does not parse: {e}", path.display()));
    let committed: BTreeSet<String> = contract["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("{} has no tools array", path.display()))
        .iter()
        .filter_map(|t| t["name"].as_str().map(str::to_string))
        .collect();

    let extra_in_registry: Vec<&String> = registry.difference(&committed).collect();
    let missing_from_registry: Vec<&String> = committed.difference(&registry).collect();
    assert!(
        extra_in_registry.is_empty() && missing_from_registry.is_empty(),
        "the registry and {} disagree (run `mcphost contract dump`): \
         in the registry but not the contract: {extra_in_registry:?}; \
         in the contract but not the registry: {missing_from_registry:?}",
        mcphost::api_contract::DEFAULT_CONTRACT_PATH
    );
}
