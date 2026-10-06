//! PRD-mcphost-ownership-moment
//! AC2 (P0) — Given an unclaimed external tenant's first successful
//! `host.tool_publish`, When the response is read, Then
//! `_meta.next.kind = "claim"` with the URL; given its second publish,
//! Then no claim nudge is present; given a synthetic tenant, Then never.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

/// Same seam `tests/attrib_ac4_external_signup_counts_real.rs` already
/// uses: `control::signup` takes a plain `source_ip: &str`, so a fake
/// external address classifies the tenant `origin: "external"` with no
/// transport-level fakery needed.
async fn signup_external(server: &TestServer, name: &str) -> (String, String) {
    let result = mcphost::control::signup(
        &server.state,
        &json!({"name": name}),
        "8.8.8.8",
        mcphost::control::SignupAttribution::default(),
    )
    .await
    .expect("external signup");
    let ns = result["tenant"].as_str().expect("tenant").to_string();
    let key = result["key"].as_str().expect("key").to_string();
    (ns, key)
}

fn publish_args(name: &str) -> serde_json::Value {
    json!({
        "name": name,
        "kind": "echo",
        "spec": {"schema": {"type": "object", "properties": {}, "required": []}},
    })
}

#[tokio::test]
async fn first_publish_by_an_unclaimed_external_tenant_carries_the_claim_nudge() {
    let server = TestServer::start().await;
    let (_ns, key) = signup_external(&server, "AC2 External Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let raw = client
        .tools_call("host.tool_publish", publish_args("hello"))
        .await
        .expect("first publish must succeed");
    let next = raw["_meta"]["next"]
        .as_object()
        .expect("_meta.next present on the first successful publish");
    assert_eq!(next["kind"], json!("claim"), "{next:?}");
    let url = next["url"].as_str().expect("_meta.next.url present");
    assert!(
        url.starts_with("https://") && url.contains("/claim/"),
        "not a claim url: {url}"
    );
    assert!(next["text"].as_str().is_some_and(|t| t.contains(url)), "{next:?}");
}

#[tokio::test]
async fn second_publish_by_the_same_tenant_carries_no_claim_nudge() {
    let server = TestServer::start().await;
    let (_ns, key) = signup_external(&server, "AC2 External Tenant Two").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let first = client
        .tools_call("host.tool_publish", publish_args("first"))
        .await
        .expect("first publish must succeed");
    assert_eq!(first["_meta"]["next"]["kind"], json!("claim"), "{first:?}");

    let second = client
        .tools_call("host.tool_publish", publish_args("second"))
        .await
        .expect("second publish must succeed");
    assert!(
        second.get("_meta").and_then(|m| m.get("next")).is_none(),
        "a second publish by the same tenant must carry no _meta.next claim nudge: {second}"
    );
}

#[tokio::test]
async fn a_synthetic_tenants_publish_is_never_nudged() {
    let server = TestServer::start().await;
    // `signup()` (tests/common) connects over the TestServer's own loopback
    // listener with no synthetic header at all -- classified `origin:
    // "synthetic"` by `classify_source_class`'s loopback rule, same tenant
    // shape every other suite's plain `signup()` helper already produces.
    let (ns, key) = signup(&server.base_url, "AC2 Synthetic Tenant").await;
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("query")
        .expect("tenant exists");
    assert_eq!(tenant.origin, "synthetic", "test setup must be synthetic");

    let client = McpClient::with_bearer(&server.base_url, &key);
    for name in ["one", "two"] {
        let raw = client
            .tools_call("host.tool_publish", publish_args(name))
            .await
            .unwrap_or_else(|e| panic!("publish '{name}' failed: {} {}", e.code, e.message));
        assert!(
            raw.get("_meta").and_then(|m| m.get("next")).is_none(),
            "a synthetic tenant must never see the claim nudge: {raw}"
        );
    }

    let nudged = server
        .state
        .db
        .claim_nudged_unix_for_test(tenant.id)
        .await
        .expect("query claim_nudged_unix");
    assert!(nudged.is_none(), "a synthetic tenant's claim_nudged_unix must stay null");
}
