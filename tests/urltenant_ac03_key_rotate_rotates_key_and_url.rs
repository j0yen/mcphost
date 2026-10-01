//! PRD-mcphost-url-bound-tenants
//! AC3 (P0) — Given a URL-bound session, When `host.key_rotate` is
//! called, Then the response carries a new key and a new URL, the old URL
//! returns 404 on the next request, and the old key is
//! `tenant_key_invalid`.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn key_rotate_rotates_both_key_and_url() {
    let server = TestServer::start().await;
    let (_ns, old_key) = common::signup(&server.base_url, "AC3 Tenant").await;

    // Mint the tenant's first URL by rotating once over the raw key --
    // "URL-bound session" below is the second rotation, called over the
    // URL itself.
    let old_key_client = McpClient::with_bearer(&server.base_url, &old_key);
    let first_rotate = extract_structured(
        &old_key_client
            .tools_call("host.key_rotate", json!({}))
            .await
            .expect("first key_rotate mints a URL"),
    );
    let old_url = first_rotate["url"].as_str().expect("url").to_string();
    let old_url_path = old_url.trim_start_matches(&server.base_url).to_string();
    let key_after_first_rotate = first_rotate["key"].as_str().expect("key").to_string();
    assert_ne!(key_after_first_rotate, old_key);

    // Given a URL-bound session: call host.key_rotate over the URL itself,
    // no Authorization header and no tenant_key.
    let url_client = McpClient::new(&server.base_url).with_path(&old_url_path);
    let second_rotate = extract_structured(
        &url_client
            .tools_call("host.key_rotate", json!({}))
            .await
            .expect("host.key_rotate over a URL-bound session"),
    );
    let new_key = second_rotate["key"].as_str().expect("response carries a new key").to_string();
    let new_url = second_rotate["url"].as_str().expect("response carries a new url").to_string();
    assert_ne!(new_key, key_after_first_rotate, "key must actually change");
    assert_ne!(new_url, old_url, "url must actually change");

    // The old URL returns 404 on the very next request.
    let http = reqwest::Client::new();
    let resp = http
        .post(format!("{}{old_url_path}", server.base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}}))
        .send()
        .await
        .expect("POST the old URL after rotation");
    assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND, "the old URL must 404 after rotation");

    // The old key (from before the second rotate) is tenant_key_invalid.
    let err = McpClient::new(&server.base_url)
        .tools_call("host.whoami", json!({"tenant_key": key_after_first_rotate}))
        .await
        .expect_err("the key that was current before the URL-bound rotation must now be invalid");
    assert_eq!(err.error_code.as_deref(), Some("tenant_key_invalid"));

    // The new key and new URL both work.
    let new_url_path = new_url.trim_start_matches(&server.base_url).to_string();
    McpClient::new(&server.base_url)
        .with_path(&new_url_path)
        .tools_call("host.whoami", json!({}))
        .await
        .expect("the new URL must authenticate");
    McpClient::new(&server.base_url)
        .tools_call("host.whoami", json!({"tenant_key": new_key}))
        .await
        .expect("the new key must authenticate");
}
