//! PRD-mcphost-session-journal-v2
//! AC1 (P0) — Given a published tool called with `{"a":1,"b":2}`,
//! `{"b":2,"a":1}`, `{"a":1,"b":3}` and no arguments, When `host.calls.list
//! {tool}` runs, Then rows 1 and 2 hold one `args_sha256`, row 3 another and
//! row 4 null, and each non-null value is `idempotency::args_sha256` of that
//! call's masked arguments.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use mcphost::idempotency::args_sha256;
use serde_json::json;

#[tokio::test]
async fn jrnl_ac01_same_arguments_share_a_fingerprint() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Fingerprint Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool.publish",
            json!({"name": "fp_echo", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    for args in [json!({"a": 1, "b": 2}), json!({"b": 2, "a": 1}), json!({"a": 1, "b": 3}), json!({})] {
        client.tools_call(&format!("{ns}.fp_echo"), args).await.expect("call");
    }

    let listing = extract_structured(
        &client
            .tools_call("host.calls.list", json!({"since": "-1h", "tool": "fp_echo"}))
            .await
            .expect("host.calls.list"),
    );
    let rows = listing["calls"].as_array().expect("calls");
    assert_eq!(rows.len(), 4, "{listing}");
    // Newest first: rows[3] is call 1, rows[0] is call 4.
    let sha = |i: usize| rows[3 - i]["args_sha256"].clone();
    assert!(sha(0).is_string(), "{listing}");
    assert_eq!(sha(0), sha(1), "key order never changes the hash: {listing}");
    assert!(sha(2).is_string() && sha(2) != sha(0), "{listing}");
    assert!(sha(3).is_null(), "a call with no arguments has no fingerprint: {listing}");
    assert_eq!(sha(0), json!(args_sha256(&json!({"a": 1, "b": 2}))), "{listing}");
    assert_eq!(sha(2), json!(args_sha256(&json!({"a": 1, "b": 3}))), "{listing}");
}
