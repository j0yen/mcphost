//! PRD-mcphost-acting-for-in-ledger-and-audit
//! AC2 (P0) — Given the same child, When its call fails with any structured
//! error, Then `error.data.acting_for` equals the object above; Given the
//! primary, Then no `acting_for` field exists on success or error.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

async fn delegate(base: &str, client: &McpClient, label: &str, ttl_s: i64) -> McpClient {
    let out: Value = extract_structured(
        &client
            .tools_call(
                "host.key.delegate",
                json!({"scopes": ["tools:call"], "ttl_s": ttl_s, "label": label, "can_delegate": true}),
            )
            .await
            .unwrap_or_else(|e| panic!("delegate {label}: {e:?}")),
    );
    McpClient::with_bearer(base, out["key"].as_str().expect("key"))
}

#[tokio::test]
async fn actfor_ac02_child_errors_carry_acting_for_and_the_primary_never_does() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "ActFor Error Tenant").await;
    let primary = McpClient::with_bearer(&server.base_url, &key);
    let lead = delegate(&server.base_url, &primary, "lead-job-42", 3000).await;
    let deployer = delegate(&server.base_url, &lead, "deployer", 2000).await;

    // Two different structured failures: a scope escalation and a missing tool.
    let escalation = deployer
        .tools_call("host.key.delegate", json!({"scopes": ["tools:write"], "ttl_s": 60, "label": "wide"}))
        .await
        .expect_err("escalation must be refused");
    let unknown = deployer.tools_call("no_such_tool_here", json!({})).await.expect_err("unknown tool");
    let ok = deployer.tools_call("host.whoami", json!({})).await.expect("whoami");
    let expected = ok["_meta"]["acting_for"].clone();
    assert_eq!(expected["label"], json!("deployer"), "{ok}");
    assert_eq!(expected["parent_label"], json!("lead-job-42"), "{ok}");
    assert_eq!(expected["root_label"], json!("lead-job-42"), "{ok}");
    assert_eq!(expected["depth"], json!(2), "{ok}");
    for err in [&escalation, &unknown] {
        assert!(err.error_code.is_some(), "{err:?}");
        assert_eq!(err.data["acting_for"], expected, "{err:?}");
    }

    // The primary: no acting_for on success or on error.
    let ok = primary.tools_call("host.whoami", json!({})).await.expect("primary whoami");
    assert!(ok.get("_meta").and_then(|m| m.get("acting_for")).is_none(), "{ok}");
    let err = primary.tools_call("no_such_tool_here", json!({})).await.expect_err("unknown tool");
    assert!(err.data.get("acting_for").is_none(), "{err:?}");
}
