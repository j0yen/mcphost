//! AC6 — Given a free owner with three public tools, When a fourth is
//! shared, Then `share_quota_exceeded` names 3.

mod common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn fourth_share_on_free_plan_is_refused() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Tenant A").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    for i in 0..3 {
        let name = format!("tool{i}");
        client
            .tools_call(
                "host.tool_publish",
                json!({"name": name, "kind": "echo", "spec": {"schema": {"type": "object"}}}),
            )
            .await
            .expect("publish ok");
        client
            .tools_call("host.tool_share", json!({"name": name, "visibility": "public"}))
            .await
            .expect("share ok");
    }

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "tool3", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish 4th tool ok (tools_max is separate from shared_tools_max)");
    let err = client
        .tools_call("host.tool_share", json!({"name": "tool3", "visibility": "public"}))
        .await
        .expect_err("sharing a 4th tool on the free plan must be refused");
    assert_eq!(err.error_code.as_deref(), Some("share_quota_exceeded"));
    assert!(
        err.message.contains('3'),
        "error message must name the limit (3): {}",
        err.message
    );
}
