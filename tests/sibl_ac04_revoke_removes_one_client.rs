//! PRD-mcphost-same-user-second-agent
//! AC4 — Given two siblings, When one is revoked via `host.client.revoke`,
//! Then its next call fails `tenant_key_invalid`, `clients` lists one row,
//! and revoking the last remaining client is refused with `last_client`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn sibl_ac04_revoke_removes_one_client() {
    let server = TestServer::start().await;
    let base = server.base_url.clone();
    let (ns, key) = signup(&base, "Revoking Host").await;
    let first = McpClient::with_bearer(&base, &key);
    let me = extract_structured(&first.tools_call("host.whoami", json!({})).await.expect("whoami"));
    let client0 = me["client_id"].as_str().expect("client_id").to_string();
    let join_path = me["join_url"].as_str().expect("join_url").trim_start_matches(&base).to_string();

    let joiner = McpClient::new(&base).with_path(&join_path).with_session_continuity();
    let joined = extract_structured(&joiner.tools_call("host.whoami", json!({})).await.expect("join"));
    let sibling = joined["client_id"].as_str().expect("sibling client_id").to_string();
    let sibling_path = joined["onboarding"]["url"].as_str().expect("url").trim_start_matches(&base).to_string();
    let sibling_client = McpClient::new(&base).with_path(&sibling_path);
    sibling_client.tools_call("host.whoami", json!({})).await.expect("the sibling's own URL works");

    // Revoke the sibling from client 0.
    let revoked = extract_structured(
        &first.tools_call("host.client.revoke", json!({"client_id": sibling})).await.expect("revoke"),
    );
    assert_eq!(revoked["revoked"], json!(sibling));

    let err = sibling_client
        .tools_call("host.whoami", json!({}))
        .await
        .expect_err("a revoked client's URL must stop working");
    assert_eq!(err.error_code.as_deref(), Some("tenant_key_invalid"), "{err:?}");
    // Its join session is dead too: the session binding does not outlive the revoke.
    let err = joiner
        .tools_call("host.whoami", json!({}))
        .await
        .expect_err("the revoked client's joined session must stop working");
    assert_eq!(err.error_code.as_deref(), Some("tenant_key_invalid"), "{err:?}");

    let after = extract_structured(&first.tools_call("host.whoami", json!({})).await.expect("whoami"));
    let clients = after["clients"].as_array().expect("clients");
    assert_eq!(clients.len(), 1, "one live row remains: {after}");
    assert_eq!(clients[0]["client_id"], json!(client0));
    assert_eq!(after["namespace"], json!(ns));

    // The last remaining client cannot be revoked.
    let err = first
        .tools_call("host.client.revoke", json!({"client_id": client0}))
        .await
        .expect_err("revoking the last client must be refused");
    assert_eq!(err.error_code.as_deref(), Some("last_client"), "{err:?}");
    first.tools_call("host.whoami", json!({})).await.expect("client 0 still works");
}
