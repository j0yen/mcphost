//! PRD-mcphost-key-delegate-child-keys
//! AC2 (P0) — Given the primary, When it delegates `{scopes: ["tools:call"],
//! tools: ["a"], ttl_s: 600, label: "c1", can_delegate: true}` and `c1`
//! delegates `{scopes: ["tools:call"], tools: ["a"], ttl_s: 300, label:
//! "c2"}`, Then both succeed, `c2.depth == 2`, and `c2` delegating anything
//! fails `permission_denied` with `data.reason == "can_delegate=false"`.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn dkeyc_ac02_primary_to_c1_to_c2_and_c2_may_not_delegate() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Delegate Chain Tenant").await;
    let primary = McpClient::with_bearer(&server.base_url, &key);
    // Tool name `a` is not publishable (names are 2+ chars) and need not exist: an
    // allowlist entry grants nothing until a tool of that name does.
    let c1 = extract_structured(
        &primary
            .tools_call(
                "host.key.delegate",
                json!({"scopes": ["tools:call"], "tools": ["a"], "ttl_s": 600, "label": "c1", "can_delegate": true}),
            )
            .await
            .expect("delegate c1"),
    );
    assert_eq!(c1["depth"], json!(1), "{c1}");
    assert_eq!(c1["parent"], json!(null), "{c1}");
    assert!(c1["key"].as_str().expect("key").starts_with("mk_"));

    let c1_client = McpClient::with_bearer(&server.base_url, c1["key"].as_str().unwrap());
    let c2 = extract_structured(
        &c1_client
            .tools_call(
                "host.key.delegate",
                json!({"scopes": ["tools:call"], "tools": ["a"], "ttl_s": 300, "label": "c2"}),
            )
            .await
            .expect("c1 delegates c2"),
    );
    assert_eq!(c2["depth"], json!(2), "{c2}");
    assert_eq!(c2["parent"], json!("c1"), "{c2}");
    assert_eq!(c2["can_delegate"], json!(false), "{c2}");
    assert_eq!(c2["tools"], json!(["a"]), "{c2}");

    let c2_client = McpClient::with_bearer(&server.base_url, c2["key"].as_str().unwrap());
    let err = c2_client
        .tools_call("host.key.delegate", json!({"scopes": ["tools:call"], "ttl_s": 60, "label": "c3"}))
        .await
        .expect_err("c2 may not delegate");
    assert_eq!(err.error_code.as_deref(), Some("permission_denied"), "{err:?}");
    assert_eq!(err.data["reason"], json!("can_delegate=false"), "{err:?}");
}
