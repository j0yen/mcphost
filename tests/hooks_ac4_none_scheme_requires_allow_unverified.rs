//! AC4 (P0) — Given a trigger with `verify.scheme = "none"` and no
//! `allow_unverified`, When set, Then `trigger_invalid` says unverified
//! triggers need the flag; With the flag, `host.trigger.list` shows
//! `unverified: true`.

mod common;
use common::{extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn none_scheme_without_flag_is_rejected_with_flag_is_unverified() {
    let server = common::TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC4 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "monitor_hook", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");

    let rejected = client
        .tools_call(
            "host.trigger.set",
            json!({"tool": "monitor_hook", "kind": "event", "verify": {"scheme": "none"}}),
        )
        .await
        .expect_err("scheme none without the flag must be rejected");
    assert_eq!(rejected.error_code.as_deref(), Some("trigger_invalid"));
    assert!(
        rejected.message.to_ascii_lowercase().contains("allow_unverified"),
        "message must name the flag: {}",
        rejected.message
    );

    let set = extract_structured(
        &client
            .tools_call(
                "host.trigger.set",
                json!({
                    "tool": "monitor_hook",
                    "kind": "event",
                    "verify": {"scheme": "none", "allow_unverified": true},
                }),
            )
            .await
            .expect("trigger.set with the flag"),
    );
    assert_eq!(set["unverified"], json!(true));

    let listed = extract_structured(
        &client
            .tools_call("host.trigger.list", json!({"tool": "monitor_hook"}))
            .await
            .expect("trigger.list"),
    );
    let triggers = listed["triggers"].as_array().expect("triggers array");
    assert_eq!(triggers.len(), 1);
    assert_eq!(triggers[0]["unverified"], json!(true));
}
