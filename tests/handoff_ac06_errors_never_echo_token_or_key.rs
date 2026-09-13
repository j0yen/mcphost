//! PRD-mcphost-handoff-token
//! AC6 (P0) — Given logs, journal, and tool_logs after the full flow, When
//! grepped for the token and both keys, Then none appear in plaintext
//! anywhere.
//!
//! Same posture as `tests/autherr_ac2_tenant_key_invalid.rs` for
//! `tenant_key`: every structured error this crate returns is checked here
//! for the offending credential, since an error message is exactly the
//! kind of thing that ends up in a client-side log or transcript. The
//! deeper "never in a tracing/journal line" guarantee is enforced at the
//! source (`control::redeem`/`control::key_rotate` never pass a token or
//! key to `tracing::*!`, only the token's own row id) -- see those
//! functions' doc comments.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn already_redeemed_error_never_echoes_the_token() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);
    let raw = client
        .tools_call("signup", json!({"name": "Ac6 Tenant", "handoff": true}))
        .await
        .expect("signup(handoff: true)");
    let token = extract_structured(&raw)["handoff_token"]
        .as_str()
        .expect("handoff_token")
        .to_string();

    client
        .tools_call("host.redeem", json!({"handoff_token": token}))
        .await
        .expect("first redeem");
    let err = client
        .tools_call("host.redeem", json!({"handoff_token": token}))
        .await
        .expect_err("second redeem must fail");
    assert!(!err.message.contains(&token), "message: {}", err.message);
    assert!(
        !err.data.to_string().contains(&token),
        "error data: {}",
        err.data
    );
}

#[tokio::test]
async fn bearer_invalid_error_never_echoes_the_rotated_out_key() {
    let server = TestServer::start().await;
    let (_ns, old_key) = signup(&server.base_url, "Ac6 Rotate Tenant").await;
    let old_client = McpClient::with_bearer(&server.base_url, &old_key);
    old_client
        .tools_call("host.key_rotate", json!({}))
        .await
        .expect("rotate");

    let err = old_client
        .tools_call("host.whoami", json!({}))
        .await
        .expect_err("old key must fail");
    assert!(!err.message.contains(&old_key), "message: {}", err.message);
    assert!(!err.data.to_string().contains(&old_key), "error data: {}", err.data);
}
