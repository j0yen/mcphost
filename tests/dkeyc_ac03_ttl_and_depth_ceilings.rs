//! PRD-mcphost-key-delegate-child-keys
//! AC3 (P0) — Given `c1` with `expires_at` in 600 s, When it delegates with
//! `ttl_s: 1200`, Then `scope_escalation` with `excess.kind == "ttl"`; Given
//! depth 3, When it delegates, Then `excess.kind == "depth"`.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

async fn delegate(client: &McpClient, label: &str, ttl_s: i64) -> Value {
    extract_structured(
        &client
            .tools_call(
                "host.key.delegate",
                json!({"scopes": ["tools:call"], "ttl_s": ttl_s, "label": label, "can_delegate": true}),
            )
            .await
            .unwrap_or_else(|e| panic!("delegate {label}: {e:?}")),
    )
}

#[tokio::test]
async fn dkeyc_ac03_child_cannot_outlive_parent_or_nest_past_depth_three() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Delegate Ceiling Tenant").await;
    let primary = McpClient::with_bearer(&server.base_url, &key);
    let c1 = delegate(&primary, "c1", 600).await;
    let c1_client = McpClient::with_bearer(&server.base_url, c1["key"].as_str().unwrap());

    let err = c1_client
        .tools_call("host.key.delegate", json!({"scopes": ["tools:call"], "ttl_s": 1200, "label": "long"}))
        .await
        .expect_err("ttl beyond the parent's expiry");
    assert_eq!(err.error_code.as_deref(), Some("scope_escalation"), "{err:?}");
    assert_eq!(err.data["excess"]["kind"], json!("ttl"), "{err:?}");

    // c1 -> c2 -> c3 is depth 3; c3 is the deepest key that may exist.
    let c2 = delegate(&c1_client, "c2", 300).await;
    let c2_client = McpClient::with_bearer(&server.base_url, c2["key"].as_str().unwrap());
    let c3 = delegate(&c2_client, "c3", 120).await;
    assert_eq!(c3["depth"], json!(3), "{c3}");
    let c3_client = McpClient::with_bearer(&server.base_url, c3["key"].as_str().unwrap());

    let err = c3_client
        .tools_call("host.key.delegate", json!({"scopes": ["tools:call"], "ttl_s": 60, "label": "c4"}))
        .await
        .expect_err("depth 4 is refused");
    assert_eq!(err.error_code.as_deref(), Some("scope_escalation"), "{err:?}");
    assert_eq!(err.data["excess"]["kind"], json!("depth"), "{err:?}");
}
