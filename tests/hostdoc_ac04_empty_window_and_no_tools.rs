//! PRD-mcphost-host-doctor-tool
//! AC4 -- Given a tenant with zero failed calls and a tenant with zero tools,
//! When `host.doctor` runs, Then both return `groups: []` with the specified
//! `next`, and the second carries `hint: host.quickstart`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

async fn doctor(client: &McpClient, args: Value) -> Value {
    extract_structured(&client.tools_call("host.doctor", args).await.expect("host.doctor"))
}

#[tokio::test]
async fn zero_failures_and_zero_tools_both_say_so_and_only_the_second_hints() {
    let server = TestServer::start().await;

    // A tenant with a tool it has called successfully: no failed calls.
    let (_, key_a) = signup(&server.base_url, "hostdoc ac4 healthy").await;
    let a = McpClient::with_bearer(&server.base_url, &key_a);
    a.tools_call(
        "host.tool_publish",
        json!({"name": "hello", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
    )
    .await
    .expect("publish");
    a.tools_call("host.tool_call", json!({"name": "hello", "args": {}})).await.expect("call hello");

    let healthy = doctor(&a, json!({})).await;
    assert_eq!(healthy["groups"], json!([]), "{healthy}");
    assert_eq!(
        healthy["next"],
        json!("no failed calls in the last 50; publish and call a tool, then ask again"),
        "{healthy}"
    );
    assert!(healthy.get("hint").is_none(), "a tenant with a tool gets no hint: {healthy}");
    assert_eq!(healthy["window"]["failed"], json!(0), "{healthy}");

    // A tenant that never published anything.
    let (ns_b, key_b) = signup(&server.base_url, "hostdoc ac4 empty").await;
    let b = McpClient::with_bearer(&server.base_url, &key_b);
    let empty = doctor(&b, json!({"limit": 10})).await;
    assert_eq!(empty["groups"], json!([]), "{empty}");
    assert_eq!(
        empty["next"],
        json!("no failed calls in the last 10; publish and call a tool, then ask again"),
        "{empty}"
    );
    assert_eq!(empty["hint"], json!("host.quickstart"), "{empty}");

    // R6: a tenant key may name only itself.
    let own = doctor(&b, json!({"tenant": ns_b})).await;
    assert_eq!(own["groups"], json!([]), "{own}");
    let other = b.tools_call("host.doctor", json!({"tenant": "someone-else"})).await.expect_err("another tenant");
    assert_eq!(other.error_code.as_deref(), Some("forbidden"), "{} {}", other.code, other.message);
}
