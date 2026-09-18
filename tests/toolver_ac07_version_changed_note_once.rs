//! AC7 — Given an unpinned sharer, When the owner rolls back (or, as
//! tested here, publishes), Then the sharer's next result carries
//! `version_changed` once and not on the following call.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn unpinned_caller_sees_version_changed_exactly_once() {
    let server = TestServer::start().await;

    let (ns_a, key_a) = signup(&server.base_url, "Tenant A").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    client_a
        .tools_call(
            "host.tool_publish",
            json!({"name": "greet", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("A publishes v1");
    client_a
        .tools_call("host.tool_share", json!({"name": "greet", "visibility": "public"}))
        .await
        .expect("A shares greet publicly");

    let (_ns_b, key_b) = signup(&server.base_url, "Tenant B").await;
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);
    let qualified = format!("{ns_a}.greet");

    // First-ever unpinned call: establishes B's baseline (v1), no note yet.
    let first = client_b
        .tools_call(&qualified, json!({"msg": "hi"}))
        .await
        .expect("B's first call");
    assert!(
        extract_structured(&first).get("version_changed").is_none(),
        "no baseline to diff against yet: {:?}",
        extract_structured(&first)
    );

    client_a
        .tools_call(
            "host.tool_publish",
            json!({"name": "greet", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("A publishes v2");

    // Next call after the change: carries the note exactly once.
    let second = client_b
        .tools_call(&qualified, json!({"msg": "hi"}))
        .await
        .expect("B's second call");
    assert_eq!(
        extract_structured(&second)["version_changed"],
        json!({"from": 1, "to": 2}),
        "must carry version_changed after A's publish: {:?}",
        extract_structured(&second)
    );

    // Following call, no further change: no note.
    let third = client_b
        .tools_call(&qualified, json!({"msg": "hi"}))
        .await
        .expect("B's third call");
    assert!(
        extract_structured(&third).get("version_changed").is_none(),
        "must not repeat on the following call: {:?}",
        extract_structured(&third)
    );
}
