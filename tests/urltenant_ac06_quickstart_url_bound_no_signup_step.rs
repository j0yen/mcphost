//! PRD-mcphost-url-bound-tenants
//! AC6 (P0) — Given a URL-bound session, When `host.quickstart kind=echo`
//! is called, Then the response has no signup step and
//! `authenticated: true`.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn quickstart_over_a_url_bound_session_skips_signup() {
    let server = TestServer::start().await;
    let session = McpClient::new(&server.base_url).with_session_continuity();
    session
        .tools_call("signup", json!({"name": "AC6 Tenant"}))
        .await
        .expect("signup");
    let rotated = extract_structured(
        &session
            .tools_call("host.key_rotate", json!({}))
            .await
            .expect("host.key_rotate mints a URL"),
    );
    let url = rotated["url"].as_str().expect("url").to_string();
    let path = url.trim_start_matches(&server.base_url).to_string();

    let url_client = McpClient::new(&server.base_url).with_path(&path);
    let quickstart = url_client
        .tools_call("host.quickstart", json!({"kind": "echo"}))
        .await
        .expect("host.quickstart over a URL-bound session, no Authorization, no tenant_key");
    let quickstart = extract_structured(&quickstart);

    assert_eq!(quickstart["authenticated"], json!(true), "{quickstart}");
    assert_eq!(quickstart["kind"], json!("echo"), "{quickstart}");

    // No signup step anywhere in the response.
    let serialized = quickstart.to_string();
    assert!(
        !serialized.contains("\"call\":\"signup\""),
        "an authenticated quickstart must carry no signup step: {quickstart}"
    );
    let steps = quickstart["steps"].as_array().expect("steps array");
    assert!(
        steps.iter().all(|s| s["call"].as_str() != Some("signup")),
        "steps must not include a signup call: {quickstart}"
    );
}
