//! PRD-mcphost-agent-mesh-ops
//! AC8 (P0) -- Given the mesh tables exist, When `/healthz` is fetched,
//! Then it contains `mesh.messages_24h`, `mesh.posts_24h` and
//! `mesh.frozen_tenants` as integers and `mcphost-deploy probe` still
//! passes.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, signup};
use serde_json::{Value, json};

async fn healthz(base_url: &str) -> (reqwest::StatusCode, Value) {
    let resp = reqwest::Client::new()
        .get(format!("{base_url}/healthz"))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .expect("GET /healthz");
    let status = resp.status();
    let body = resp.json().await.expect("parse /healthz");
    (status, body)
}

#[tokio::test]
async fn ac8_healthz_reports_mesh_counters_as_integers() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "Tenant A").await;
    let (ns_b, _key_b) = signup(&server.base_url, "Tenant B").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    client_a
        .tools_call("host.msg.send", json!({"to": [ns_b], "body": "hi"}))
        .await
        .expect("A sends a message");
    client_a
        .tools_call("host.channel.open", json!({"name": "c1"}))
        .await
        .expect("A opens a channel");
    client_a
        .tools_call("host.channel.post", json!({"channel": "c1", "body": "post"}))
        .await
        .expect("A posts");
    admin
        .tools_call("admin.mesh.freeze", json!({"tenant": ns_a, "reason": "test"}))
        .await
        .expect("admin.mesh.freeze");

    // `mcphost-deploy probe` itself just needs `/healthz` to answer 200
    // with parseable JSON that still carries the pre-existing keys it
    // already reads (PRD's own migration/compatibility note: "additive
    // JSON... existing probes ignore unknown keys").
    let (status, body) = healthz(&server.base_url).await;
    assert!(status.is_success(), "{status}");
    assert!(body["tenants_total"].is_i64(), "{body:?}");
    assert!(body["tools_total"].is_i64(), "{body:?}");

    let mesh = &body["mesh"];
    assert!(mesh["messages_24h"].is_i64(), "{body:?}");
    assert!(mesh["posts_24h"].is_i64(), "{body:?}");
    assert!(mesh["frozen_tenants"].is_i64(), "{body:?}");
    assert_eq!(mesh["messages_24h"], json!(1), "{body:?}");
    assert_eq!(mesh["posts_24h"], json!(1), "{body:?}");
    assert_eq!(mesh["frozen_tenants"], json!(1), "{body:?}");
}
