//! PRD-mcphost-org-model
//! AC1 — Given a tenant with 2 published tools and a `/u/<secret>/mcp` URL,
//! When it calls `host.org.join {code}` for a fresh org, Then
//! `tenants.key_hash`, `namespace`, its `tools` rows and its URL's
//! `tools/list` are byte-identical before and after, and
//! `host.whoami.org.role == "member"`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, publish, signup};
use serde_json::json;

fn echo_spec() -> serde_json::Value {
    json!({"schema": {"type": "object", "properties": {}, "required": []}})
}

#[tokio::test]
async fn orgm_ac01_join_keeps_key_namespace_tools_and_url_listing() {
    let server = TestServer::start().await;

    // The org owner makes a fresh org and hands out its code.
    let (_owner_ns, owner_key) = signup(&server.base_url, "Org Owner").await;
    let owner = McpClient::with_bearer(&server.base_url, &owner_key);
    let created = extract_structured(
        &owner
            .tools_call("host.org.create", json!({"name": "acme"}))
            .await
            .expect("host.org.create"),
    );
    assert_eq!(created["role"], json!("owner"), "{created}");
    let invite_url = created["invite_url"].as_str().expect("invite_url");
    assert!(invite_url.contains("/o/") && invite_url.ends_with("/mcp"), "{invite_url}");
    let code = invite_url
        .trim_end_matches("/mcp")
        .rsplit('/')
        .next()
        .expect("code segment")
        .to_string();

    // An existing tenant with two tools and a URL-bound session.
    let session = McpClient::new(&server.base_url).with_session_continuity();
    let signed = extract_structured(
        &session
            .tools_call("signup", json!({"name": "Joiner"}))
            .await
            .expect("signup"),
    );
    let ns = signed["tenant"].as_str().expect("namespace").to_string();
    let key = signed["key"].as_str().expect("tenant_key").to_string();
    let bearer = McpClient::with_bearer(&server.base_url, &key);
    publish(&bearer, "alpha", "echo", echo_spec()).await;
    publish(&bearer, "beta", "echo", echo_spec()).await;
    let rotated = extract_structured(
        &session
            .tools_call("host.key_rotate", json!({}))
            .await
            .expect("host.key_rotate mints a URL"),
    );
    let url_path = rotated["url"]
        .as_str()
        .expect("url")
        .trim_start_matches(&server.base_url)
        .to_string();
    let url_client = McpClient::new(&server.base_url).with_path(&url_path);

    let snapshot = || async {
        let tenant = server
            .state
            .db
            .find_tenant_by_namespace(ns.clone())
            .await
            .expect("tenant lookup")
            .expect("tenant exists");
        let tools = server.state.db.list_tools(tenant.id).await.expect("tools");
        let listing = url_client.tools_list().await.expect("tools/list");
        (tenant.key_hash, tenant.namespace, format!("{tools:?}"), listing.to_string(), tools.len())
    };
    let before = snapshot().await;
    assert_eq!(before.4, 2, "fixture must carry two published tools");

    let joined = extract_structured(
        &url_client
            .tools_call("host.org.join", json!({"code": code}))
            .await
            .expect("host.org.join with the tenant's own URL"),
    );
    assert_eq!(joined["role"], json!("member"), "{joined}");

    let after = snapshot().await;
    assert_eq!(before.0, after.0, "key_hash must be byte-identical");
    assert_eq!(before.1, after.1, "namespace must be byte-identical");
    assert_eq!(before.2, after.2, "tools rows must be byte-identical");
    assert_eq!(before.3, after.3, "tools/list on the URL must be byte-identical");

    let whoami = extract_structured(
        &url_client.tools_call("host.whoami", json!({})).await.expect("host.whoami"),
    );
    assert_eq!(whoami["org"]["role"], json!("member"), "{whoami}");
    assert_eq!(whoami["org"]["name"], json!("acme"), "{whoami}");
}
