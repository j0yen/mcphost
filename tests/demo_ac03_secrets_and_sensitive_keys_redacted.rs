//! PRD-mcphost-demo-url-per-tool
//! AC3 (P0) — Given a run whose args contain `api_key` and whose result
//! contains a `{{ secret.gh }}` value, When the page renders, Then the HTML
//! contains `•••` for that key, does not contain the planted secret value,
//! and does not contain the string `{{ secret.`.

use crate::common;
use common::{TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn demo_page_masks_sensitive_keys_and_secret_values() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Demo AC3").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call("host.secret_set", json!({"name": "gh", "value": "ghp_PLANTED_SECRET_VALUE"}))
        .await
        .expect("secret_set");
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "echo", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    let shared = extract_structured(
        &client
            .tools_call("host.tool_share", json!({"name": "echo", "visibility": "url"}))
            .await
            .expect("share"),
    );
    let url = shared["url"].as_str().expect("url").to_string();

    let http = reqwest::Client::new();
    let resp = http
        .post(&url)
        .header("content-type", "application/json")
        .body(
            json!({
                "api_key": "sk-PLANTED-API-KEY",
                "text": "auth {{ secret.gh }} then ghp_PLANTED_SECRET_VALUE"
            })
            .to_string(),
        )
        .send()
        .await
        .expect("POST /x");
    assert_eq!(resp.status(), 200);

    let html = http
        .get(format!("{}/d/{}/echo", server.base_url, ns))
        .send()
        .await
        .expect("GET /d")
        .text()
        .await
        .unwrap();
    assert!(html.contains("•••"), "mask present: {html}");
    assert!(!html.contains("sk-PLANTED-API-KEY"), "api_key value leaked: {html}");
    assert!(!html.contains("ghp_PLANTED_SECRET_VALUE"), "secret value leaked: {html}");
    assert!(!html.contains("{{ secret."), "interpolation leaked: {html}");
    // The api_key key itself is shown, its value masked.
    assert!(html.contains("api_key"), "key name still shown: {html}");
}
