//! PRD-mcphost-agent-mesh-ops
//! AC2 (P0) -- Given a thread between A and B, When the admin calls
//! `admin.mesh.threads(tenant=A)`, Then the thread appears with both
//! addresses, `message_count`, and `last_activity`, and no response field
//! contains a body.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

/// Walks the whole response tree looking for a string value byte-equal to
/// `needle` (the message body A actually sent) -- a stronger check than
/// merely asserting no `"body"` key exists, since a body could in
/// principle leak under a different key name.
fn contains_string(value: &Value, needle: &str) -> bool {
    match value {
        Value::String(s) => s == needle,
        Value::Array(items) => items.iter().any(|v| contains_string(v, needle)),
        Value::Object(map) => map.values().any(|v| contains_string(v, needle)),
        _ => false,
    }
}

fn has_key(value: &Value, key: &str) -> bool {
    match value {
        Value::Object(map) => map.keys().any(|k| k == key) || map.values().any(|v| has_key(v, key)),
        Value::Array(items) => items.iter().any(|v| has_key(v, key)),
        _ => false,
    }
}

#[tokio::test]
async fn ac2_threads_lists_participants_and_counts_with_no_body() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "Agent A").await;
    let (ns_b, _key_b) = signup(&server.base_url, "Agent B").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);

    let secret_body = "this body must never leak to admin.mesh.threads";
    client_a
        .tools_call("host.msg.send", json!({"to": [ns_b], "body": secret_body}))
        .await
        .expect("A sends to B");

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let raw = admin
        .tools_call("admin.mesh.threads", json!({"tenant": ns_a}))
        .await
        .expect("admin.mesh.threads");
    let result = extract_structured(&raw);

    assert!(
        !contains_string(&result, secret_body),
        "response leaked a message body: {result:?}"
    );
    // A stricter, structural check per AC2's own wording: no key literally
    // named "body" anywhere in the tree.
    assert!(!has_key(&result, "body"), "response carries a body field: {result:?}");

    let threads = result["threads"].as_array().expect("threads array");
    assert_eq!(threads.len(), 1, "{threads:?}");
    let thread = &threads[0];
    let participants: Vec<String> = thread["participants"]
        .as_array()
        .expect("participants array")
        .iter()
        .map(|v| v.as_str().expect("participant string").to_string())
        .collect();
    assert!(participants.contains(&ns_a), "{participants:?}");
    assert!(participants.contains(&ns_b), "{participants:?}");
    assert_eq!(thread["message_count"], json!(1), "{thread:?}");
    assert!(thread["last_activity"].is_string(), "{thread:?}");
}
