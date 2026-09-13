//! PRD-mcphost-handoff-token
//! AC3 (P0) — Given a redeemed session, When `host.key_rotate` is called,
//! Then a new key returns, the old key fails on the next tool call as
//! unauthenticated, and the new key succeeds.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn rotate_issues_new_key_and_kills_the_old_one() {
    let server = TestServer::start().await;
    let (_ns, old_key) = signup(&server.base_url, "Rotate Tenant").await;

    let old_client = McpClient::with_bearer(&server.base_url, &old_key);
    let raw = old_client
        .tools_call("host.key_rotate", json!({}))
        .await
        .expect("key_rotate should succeed while authenticated with the current key");
    let new_key = extract_structured(&raw)["key"]
        .as_str()
        .expect("key_rotate returns a new key")
        .to_string();
    assert_ne!(new_key, old_key);

    // The presented (now old) key must fail every subsequent call.
    let err = old_client
        .tools_call("host.whoami", json!({}))
        .await
        .expect_err("the old key must fail after rotation");
    assert_eq!(err.error_code.as_deref(), Some("bearer_invalid"));

    // The new key must work.
    let new_client = McpClient::with_bearer(&server.base_url, &new_key);
    new_client
        .tools_call("host.whoami", json!({}))
        .await
        .expect("the new key must authenticate");
}
