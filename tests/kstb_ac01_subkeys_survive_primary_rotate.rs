//! PRD-mcphost-key-scoped-to-tools-b
//! AC1 (P0) — Given two sub-keys, When the primary calls `host.key.rotate`,
//! Then both sub-keys still authenticate and `host.key.list` shows them
//! unchanged.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn kstb_ac01_both_subkeys_survive_primary_rotate_and_list_is_unchanged() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Kstb Rotate Tenant").await;
    let primary = McpClient::with_bearer(&server.base_url, &key);
    primary
        .tools_call(
            "host.tool.publish",
            json!({"name": "ta", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish ta");
    let qualified = format!("{ns}.ta");

    let mut subs = Vec::new();
    for label in ["one", "two"] {
        let m = extract_structured(
            &primary
                .tools_call("host.key.mint", json!({"tools": ["ta"], "ttl_s": 3600, "label": label}))
                .await
                .expect("mint"),
        );
        subs.push(m["key"].as_str().expect("minted key").to_string());
    }
    let before = extract_structured(&primary.tools_call("host.key.list", json!({})).await.expect("list before"));

    let rotated = extract_structured(&primary.tools_call("host.key.rotate", json!({})).await.expect("rotate"));
    assert_ne!(rotated["key"].as_str().expect("new key"), key);
    assert!(
        rotated["usage"].as_str().unwrap_or_default().contains("Sub-keys"),
        "rotate usage must say sub-keys are untouched: {rotated}"
    );

    for sub in &subs {
        McpClient::with_bearer(&server.base_url, sub)
            .tools_call(&qualified, json!({}))
            .await
            .expect("sub-key still authenticates after the primary rotated");
    }

    let new_primary = McpClient::with_bearer(&server.base_url, rotated["key"].as_str().unwrap());
    let after = extract_structured(&new_primary.tools_call("host.key.list", json!({})).await.expect("list after"));
    let strip = |v: &serde_json::Value| {
        let mut keys = v["keys"].clone();
        for k in keys.as_array_mut().expect("keys array") {
            // `last_used_at` legitimately moves when a sub-key is used.
            k.as_object_mut().unwrap().remove("last_used_at");
        }
        keys
    };
    assert_eq!(strip(&before), strip(&after), "host.key.list must show both sub-keys unchanged");
    assert_eq!(strip(&after).as_array().unwrap().len(), 2);
}
