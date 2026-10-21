//! AC2 — Given tenant A with two open listen streams and tenant B with one,
//! When A calls `host.tool.publish` and then the alias `host.tool_publish` for
//! a second tool, Then each A stream receives two
//! `notifications/tools/list_changed` carrying its own subscription id, each
//! within 5 s of the call returning, B receives none within 5 s, a failed
//! publish on A sends none, and A's next `tools/list` shows both tools.

use crate::common;
use common::{ListenStream, McpClient, TestServer, signup};
use serde_json::{Value, json};
use std::time::Duration;

const SUB_ID: &str = "io.modelcontextprotocol/subscriptionId";

fn echo_spec() -> Value {
    json!({"schema": {"type": "object", "properties": {"msg": {"type": "string"}}}})
}

async fn open_acked(base: &str, key: &str, id: u64) -> ListenStream {
    let mut stream = ListenStream::open(base, key, id).await;
    assert_eq!(stream.status, 200, "listen {id} is accepted");
    let (method, _) = stream.next_method(Duration::from_secs(5)).await.expect("acknowledgement");
    assert_eq!(method, "notifications/subscriptions/acknowledged");
    stream
}

async fn expect_list_changed(stream: &mut ListenStream, what: &str) {
    let (method, msg) = stream
        .next_method(Duration::from_secs(5))
        .await
        .unwrap_or_else(|| panic!("stream {} got no notification within 5 s for {what}", stream.request_id));
    assert_eq!(method, "notifications/tools/list_changed", "{what}");
    assert_eq!(msg["params"]["_meta"][SUB_ID], stream.request_id, "{what}: carries its own subscription id");
}

#[tokio::test]
async fn sublis_ac02_publish_and_its_alias_notify_only_the_callers_streams() {
    let server = TestServer::start().await;
    let (_a, key_a) = signup(&server.base_url, "sublis2a").await;
    let (_b, key_b) = signup(&server.base_url, "sublis2b").await;
    let mut a1 = open_acked(&server.base_url, &key_a, 101).await;
    let mut a2 = open_acked(&server.base_url, &key_a, 102).await;
    let mut b1 = open_acked(&server.base_url, &key_b, 201).await;
    let client = McpClient::with_bearer(&server.base_url, &key_a);

    client
        .tools_call("host.tool.publish", json!({"name": "first", "kind": "echo", "spec": echo_spec()}))
        .await
        .expect("canonical publish");
    expect_list_changed(&mut a1, "host.tool.publish").await;
    expect_list_changed(&mut a2, "host.tool.publish").await;

    client
        .tools_call("host.tool_publish", json!({"name": "second", "kind": "echo", "spec": echo_spec()}))
        .await
        .expect("alias publish");
    expect_list_changed(&mut a1, "host.tool_publish").await;
    expect_list_changed(&mut a2, "host.tool_publish").await;

    // A failed publish commits nothing and sends nothing.
    let failed = client
        .tools_call("host.tool.publish", json!({"name": "bad name!", "kind": "echo", "spec": echo_spec()}))
        .await;
    assert!(failed.is_err(), "an invalid tool name is refused");
    assert!(a1.next_message(Duration::from_secs(2)).await.is_none(), "a failed publish notifies stream 1");
    assert!(a2.next_message(Duration::from_secs(2)).await.is_none(), "a failed publish notifies stream 2");

    assert!(
        b1.next_message(Duration::from_secs(5)).await.is_none(),
        "tenant B hears nothing about tenant A's tools"
    );

    let listed = client.tools_list().await.expect("tools/list");
    let names: Vec<&str> = listed["tools"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(names.iter().any(|n| n.ends_with(".first")), "first is listed: {names:?}");
    assert!(names.iter().any(|n| n.ends_with(".second")), "second is listed: {names:?}");
}
