//! PRD-mcphost-key-scoped-to-tools-b
//! AC3 (P1) — Given a Free tenant with 5 active sub-keys, When it mints a
//! sixth, Then `plan_required` naming `sub_keys_max`, and `host.quickstart`
//! `limits.plan.sub_keys_max == 5`.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn kstb_ac03_sixth_free_sub_key_is_plan_required_naming_sub_keys_max() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Kstb Ceiling Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool.publish",
            json!({"name": "ta", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish ta");

    for i in 0..5 {
        client
            .tools_call("host.key.mint", json!({"tools": ["ta"], "label": format!("k{i}")}))
            .await
            .expect("mint within the ceiling");
    }
    let err = client
        .tools_call("host.key.mint", json!({"tools": ["ta"], "label": "k5"}))
        .await
        .expect_err("sixth mint refused");
    assert_eq!(err.error_code.as_deref(), Some("plan_required"), "{err:?}");
    assert!(err.message.contains("sub_keys_max"), "message must name sub_keys_max: {err:?}");

    // A revoked sub-key frees a slot: the ceiling counts active keys.
    client
        .tools_call("host.key.revoke", json!({"label": "k0"}))
        .await
        .expect("revoke k0");
    client
        .tools_call("host.key.mint", json!({"tools": ["ta"], "label": "k5"}))
        .await
        .expect("mint after a revoke");

    let qs = extract_structured(
        &client.tools_call("host.quickstart", json!({"kind": "echo"})).await.expect("quickstart"),
    );
    assert_eq!(qs["limits"]["plan"]["sub_keys_max"], json!(5));
}
