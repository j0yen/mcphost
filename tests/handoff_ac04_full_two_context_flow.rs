//! PRD-mcphost-handoff-token
//! AC4 (P0), Joe: "test it" — Given the full two-context integration test
//! (sign up handoff, redeem, rotate, replay old token, replay old key),
//! When it runs, Then every replayed credential from the transcript fails
//! and every live-path call succeeds.
//!
//! "Context A" below stands in for one signup session's whole transcript:
//! everything it ever saw (the handoff_token, then the key redeem
//! returned, then the key host.key_rotate returned) is replayed exactly as
//! a leaked transcript would replay it, and every stale credential in that
//! replay must fail.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn signup_redeem_rotate_replay_old_token_replay_old_key() {
    let server = TestServer::start().await;

    // 1. Context A signs up in handoff mode.
    let signup_client = McpClient::new(&server.base_url);
    let signup_raw = signup_client
        .tools_call("signup", json!({"name": "Context A", "handoff": true}))
        .await
        .expect("context A signup(handoff: true)");
    let signup_result = extract_structured(&signup_raw);
    assert!(signup_result.get("key").is_none(), "signup must not leak the key");
    let handoff_token = signup_result["handoff_token"]
        .as_str()
        .expect("handoff_token")
        .to_string();

    // 2. Context A redeems the token for its tenant key.
    let redeem_raw = signup_client
        .tools_call("host.redeem", json!({"handoff_token": handoff_token}))
        .await
        .expect("context A redeem");
    let key_v1 = extract_structured(&redeem_raw)["key"]
        .as_str()
        .expect("redeemed key")
        .to_string();

    // Sanity: the freshly redeemed key works.
    let client_v1 = McpClient::with_bearer(&server.base_url, &key_v1);
    client_v1
        .tools_call("host.whoami", json!({}))
        .await
        .expect("key_v1 authenticates right after redemption");

    // 3. A replay of context A's transcript: the token appears there too
    // (it was in the signup response) -- redeeming it again must fail.
    let replay_token = signup_client
        .tools_call("host.redeem", json!({"handoff_token": handoff_token}))
        .await
        .expect_err("replaying the handoff token must fail");
    assert_eq!(replay_token.error_code.as_deref(), Some("handoff_token_redeemed"));
    assert!(!replay_token.message.contains(&handoff_token));

    // 4. Context A rotates its key.
    let rotate_raw = client_v1
        .tools_call("host.key_rotate", json!({}))
        .await
        .expect("context A key_rotate");
    let key_v2 = extract_structured(&rotate_raw)["key"]
        .as_str()
        .expect("rotated key")
        .to_string();
    assert_ne!(key_v1, key_v2);

    // 5. A replay of the pre-rotation key (also in the transcript) must
    // now fail as unauthenticated.
    let replay_key = client_v1
        .tools_call("host.whoami", json!({}))
        .await
        .expect_err("replaying the pre-rotation key must fail");
    assert_eq!(replay_key.error_code.as_deref(), Some("bearer_invalid"));

    // 6. The live path -- the new key -- still works.
    let client_v2 = McpClient::with_bearer(&server.base_url, &key_v2);
    client_v2
        .tools_call("host.whoami", json!({}))
        .await
        .expect("key_v2 authenticates after rotation");
    client_v2
        .tools_call(
            "host.tool_publish",
            json!({"name": "hello", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("the live key can still do real tenant work");
}
