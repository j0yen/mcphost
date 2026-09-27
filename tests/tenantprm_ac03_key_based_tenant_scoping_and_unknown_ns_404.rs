//! PRD-mcphost-tenant-resource-metadata
//! AC3 (P0) — Given T's tenant key, When it calls `/t/acme/mcp`, Then it
//! runs as T; When U's key calls `/t/acme/mcp`, Then 401 `wrong_tenant`;
//! When any credential calls `/t/nope/mcp`, Then 404 identical in body and
//! headers to `GET /t/nope/nothing`.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

async fn bare_call(client: &McpClient, name: &str, args: serde_json::Value) -> reqwest::Response {
    client
        .post_with_mcp_name_override(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {"name": name, "arguments": args},
            }),
            name,
        )
        .await
}

#[tokio::test]
async fn tenant_key_runs_as_that_tenant_on_its_own_path() {
    let server = TestServer::start().await;
    let (ns_t, key_t) = signup(&server.base_url, "Tenant T").await;
    let client = McpClient::with_bearer(&server.base_url, &key_t).with_path(&format!("/t/{ns_t}/mcp"));

    let result = client
        .tools_call("host.whoami", json!({}))
        .await
        .expect("T's own key on its own tenant path must succeed");
    let whoami = common::extract_structured(&result);
    assert_eq!(whoami["tenant"], json!(ns_t));
}

#[tokio::test]
async fn another_tenants_key_on_this_tenants_path_is_wrong_tenant() {
    let server = TestServer::start().await;
    let (ns_t, _key_t) = signup(&server.base_url, "Tenant T").await;
    let (_ns_u, key_u) = signup(&server.base_url, "Tenant U").await;
    let client = McpClient::with_bearer(&server.base_url, &key_u).with_path(&format!("/t/{ns_t}/mcp"));

    let resp = bare_call(&client, "host.state.get", json!({"key": "k"})).await;
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
    let body: serde_json::Value = resp.json().await.expect("parse wrong_tenant error body");
    assert_eq!(body["error"]["data"]["error_code"], json!("wrong_tenant"));
}

#[tokio::test]
async fn unknown_namespace_mcp_path_404s_identically_to_any_unmapped_route() {
    let server = TestServer::start().await;
    let (_ns_t, key_t) = signup(&server.base_url, "Tenant T").await;

    let http = reqwest::Client::new();

    // Any credential (including a real, valid one for a different tenant),
    // or none at all, gets the same 404 for an unknown namespace.
    let with_key = http
        .post(format!("{}/t/nope/mcp", server.base_url))
        .header("Authorization", format!("Bearer {key_t}"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}}))
        .send()
        .await
        .expect("POST unknown tenant path with a key");
    let no_credential = http
        .post(format!("{}/t/nope/mcp", server.base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}}))
        .send()
        .await
        .expect("POST unknown tenant path with no credential");
    let baseline = http
        .get(format!("{}/t/nope/nothing", server.base_url))
        .send()
        .await
        .expect("GET a genuinely unmapped route");

    assert_eq!(baseline.status(), reqwest::StatusCode::NOT_FOUND);
    let baseline_status = baseline.status();
    let baseline_content_type = baseline.headers().get(reqwest::header::CONTENT_TYPE).cloned();
    let baseline_body = baseline.bytes().await.expect("read baseline body");
    assert!(baseline_body.is_empty(), "axum's default 404 body must be empty: {baseline_body:?}");

    for (label, resp) in [("with_key", with_key), ("no_credential", no_credential)] {
        assert_eq!(resp.status(), baseline_status, "{label}: status must match the unmapped-route baseline");
        assert_eq!(
            resp.headers().get(reqwest::header::CONTENT_TYPE).cloned(),
            baseline_content_type,
            "{label}: Content-Type must match the unmapped-route baseline"
        );
        let body = resp.bytes().await.expect("read body");
        assert_eq!(body, baseline_body, "{label}: body must match the unmapped-route baseline");
    }
}
