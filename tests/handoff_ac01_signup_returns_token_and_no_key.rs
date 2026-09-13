//! PRD-mcphost-handoff-token
//! AC1 (P0) — Given signup with handoff mode, When it completes, Then the
//! response carries a token and expiry and no key.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn handoff_signup_returns_token_and_expiry_never_a_key() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let raw = client
        .tools_call("signup", json!({"name": "Handoff Tenant", "handoff": true}))
        .await
        .expect("signup(handoff: true) should succeed");
    let result = extract_structured(&raw);

    assert!(
        result.get("handoff_token").and_then(|v| v.as_str()).is_some(),
        "handoff signup must return a handoff_token: {result}"
    );
    assert!(
        result.get("expires_in").and_then(|v| v.as_i64()).is_some(),
        "handoff signup must return an expires_in: {result}"
    );
    assert!(
        result.get("key").is_none(),
        "handoff signup must never return the raw key: {result}"
    );
    assert_eq!(result["next"].as_str(), Some("host.redeem"));
}
