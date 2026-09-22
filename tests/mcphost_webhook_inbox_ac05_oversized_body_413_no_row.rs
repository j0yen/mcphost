//! PRD-mcphost-webhook-inbox
//! AC5 (P0) — Given a 300 KB body, When POSTed, Then 413 and no row.

use crate::common;
use common::{extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn oversized_body_is_413_with_no_row_stored() {
    let server = common::TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC5 Tenant").await;
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

    // 300 KB, over the plan's 256 KiB event_body_bytes_max limit.
    let big_body = vec![b'x'; 300 * 1024];

    let http = reqwest::Client::new();
    let resp = http
        .post(&url)
        .header("X-Mcphost-Signature", "sha256=irrelevant-size-checked-first")
        .body(big_body)
        .send()
        .await
        .expect("POST /hook/... oversized");
    assert_eq!(resp.status(), 413);

    let queried = extract_structured(
        &client
            .tools_call("host.state.query", json!({"table": "inbox_pay"}))
            .await
            .expect("state.query"),
    );
    let rows = queried["rows"].as_array().expect("rows array");
    assert!(rows.is_empty(), "an oversized delivery must store no row: {rows:?}");
}
