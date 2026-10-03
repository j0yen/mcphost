//! PRD-mcphost-public-tool-url
//! AC7 (P0) — Given a tenant paused by the admin kill switch, When its
//! public URL is called, Then 503 is returned and no run is created.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn admin_disabled_tenants_public_url_answers_503_with_no_run() {
    let server = TestServer::start().await;
    let (tenant_ns, key) = signup(&server.base_url, "AC7 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "echo", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish ok");
    let shared = extract_structured(
        &client
            .tools_call("host.tool_share", json!({"name": "echo", "visibility": "url"}))
            .await
            .expect("tool_share ok"),
    );
    let url = shared["url"].as_str().expect("url field").to_string();

    admin
        .tools_call("admin.tenant_disable", json!({"tenant": tenant_ns.clone()}))
        .await
        .expect("admin.tenant_disable");

    let runs_before = extract_structured(
        &admin
            .tools_call("admin.runs", json!({"tenant": tenant_ns.clone(), "limit": 200}))
            .await
            .expect("admin.runs"),
    )["runs"]
        .as_array()
        .expect("runs array")
        .len();

    let http = reqwest::Client::new();
    let resp = http.get(&url).send().await.expect("GET");
    assert_eq!(resp.status(), 503, "a paused tenant's public URL must answer 503");

    let runs_after = extract_structured(
        &admin
            .tools_call("admin.runs", json!({"tenant": tenant_ns, "limit": 200}))
            .await
            .expect("admin.runs"),
    )["runs"]
        .as_array()
        .expect("runs array")
        .len();
    assert_eq!(runs_before, runs_after, "a 503 from a paused tenant must not create a run");
}
