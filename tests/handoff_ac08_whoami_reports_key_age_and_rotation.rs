//! PRD-mcphost-handoff-token
//! AC8 (P1) — Given a rotated tenant, When `host.whoami` is called, Then
//! key age and last-rotation time are reported.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn whoami_reports_null_rotation_before_any_rotate() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Whoami Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let raw = client.tools_call("host.whoami", json!({})).await.expect("whoami");
    let result = extract_structured(&raw);
    assert!(result["key_rotated_at"].is_null(), "result: {result}");
    // A never-rotated key is as old as the tenant -- still a real, present
    // number, not null.
    assert!(result["key_age_s"].as_i64().is_some(), "result: {result}");
}

#[tokio::test]
async fn whoami_reports_rotation_time_after_key_rotate() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Whoami Rotate Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let before = mcphost::state::now_unix();
    let rotate_raw = client
        .tools_call("host.key_rotate", json!({}))
        .await
        .expect("key_rotate");
    let new_key = extract_structured(&rotate_raw)["key"]
        .as_str()
        .expect("new key")
        .to_string();

    let new_client = McpClient::with_bearer(&server.base_url, &new_key);
    let raw = new_client
        .tools_call("host.whoami", json!({}))
        .await
        .expect("whoami after rotation");
    let result = extract_structured(&raw);
    let rotated_at = result["key_rotated_at"]
        .as_i64()
        .expect("key_rotated_at must be set after a rotation");
    assert!(rotated_at >= before, "result: {result}");
    let age = result["key_age_s"].as_i64().expect("key_age_s");
    assert!((0..60).contains(&age), "a just-rotated key's age should be small: {age}");
}
