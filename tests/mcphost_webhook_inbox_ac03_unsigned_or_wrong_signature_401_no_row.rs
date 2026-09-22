//! PRD-mcphost-webhook-inbox
//! AC3 (P0) — Given an unsigned or wrongly signed body, When POSTed, Then
//! 401 and no row.

use crate::common;
use common::{extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn unsigned_and_wrong_signature_are_both_401_with_no_row_stored() {
    let server = common::TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC3 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "on_payment", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    let set = extract_structured(
        &client
            .tools_call(
                "host.trigger.set",
                json!({"tool": "on_payment", "kind": "webhook", "name": "pay"}),
            )
            .await
            .expect("trigger.set"),
    );
    let url = set["url"].as_str().expect("url").to_string();

    let body = json!({"amount": 100});
    let http = reqwest::Client::new();

    // Unsigned: no X-Mcphost-Signature header at all.
    let resp = http
        .post(&url)
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .expect("POST /hook/... unsigned");
    assert_eq!(resp.status(), 401);

    // Wrongly signed.
    let resp2 = http
        .post(&url)
        .header("X-Mcphost-Signature", "sha256=deadbeef")
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .expect("POST /hook/... wrong signature");
    assert_eq!(resp2.status(), 401);

    let queried = extract_structured(
        &client
            .tools_call("host.state.query", json!({"table": "inbox_pay"}))
            .await
            .expect("state.query"),
    );
    let rows = queried["rows"].as_array().expect("rows array");
    assert!(rows.is_empty(), "an unsigned or wrongly signed delivery must store no row: {rows:?}");
}
