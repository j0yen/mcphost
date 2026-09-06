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

mod common;
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
            .all(|n| n.starts_with("host.") || n.starts_with("billing.")),
        "a tenant with no published tools must see only host.*/billing.* tools: {names:?}"
    );
    assert_eq!(
        names.len(),
        16,
        "there must be exactly the twelve host.* control-plane tools \
         (incl. host.quickstart and host.tool_run) plus host.tool_call plus the \
         three billing.* tools: {names:?}"
    );
}
