//! PRD-mcphost-key-delegate-child-keys
//! AC6 (P0) — Given a Free tenant with 10 live delegated keys, When it
//! delegates an eleventh, Then `plan_required` names `delegated_keys_max` and
//! `host.quickstart` `limits.plan.delegated_keys_max == 10`; Given `ttl_s:
//! 86401`, Then `plan_required` names `delegated_ttl_max_s`.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn dkeyc_ac06_free_plan_caps_live_delegated_keys_and_their_ttl() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Delegate Plan Tenant").await;
    let primary = McpClient::with_bearer(&server.base_url, &key);
    for i in 0..10 {
        primary
            .tools_call(
                "host.key.delegate",
                json!({"scopes": ["tools:call"], "ttl_s": 600, "label": format!("k{i}")}),
            )
            .await
            .unwrap_or_else(|e| panic!("delegate k{i}: {e:?}"));
    }
    let err = primary
        .tools_call("host.key.delegate", json!({"scopes": ["tools:call"], "ttl_s": 600, "label": "k10"}))
        .await
        .expect_err("eleventh key");
    assert_eq!(err.error_code.as_deref(), Some("plan_required"), "{err:?}");
    assert_eq!(err.data["field"], json!("delegated_keys_max"), "{err:?}");
    assert!(err.message.contains("delegated_keys_max"), "{err:?}");

    let quickstart =
        extract_structured(&primary.tools_call("host.quickstart", json!({"kind": "echo"})).await.expect("quickstart"));
    assert_eq!(quickstart["limits"]["plan"]["delegated_keys_max"], json!(10), "{quickstart}");
    assert_eq!(quickstart["limits"]["plan"]["delegated_ttl_max_s"], json!(86400), "{quickstart}");

    // A revoked key frees a slot; the ttl ceiling is checked on its own.
    primary.tools_call("host.key.revoke", json!({"label": "k0"})).await.expect("revoke k0");
    let err = primary
        .tools_call("host.key.delegate", json!({"scopes": ["tools:call"], "ttl_s": 86401, "label": "long"}))
        .await
        .expect_err("ttl above the plan ceiling");
    assert_eq!(err.error_code.as_deref(), Some("plan_required"), "{err:?}");
    assert_eq!(err.data["field"], json!("delegated_ttl_max_s"), "{err:?}");
    primary
        .tools_call("host.key.delegate", json!({"scopes": ["tools:call"], "ttl_s": 86400, "label": "edge"}))
        .await
        .expect("86400 is allowed once a slot is free");
}
