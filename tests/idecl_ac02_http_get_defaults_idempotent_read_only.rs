//! PRD-mcphost-tool-idempotent-declaration AC2 -- Given an http tool
//! published with method `GET` and no flag, When `tools/list` runs, Then its
//! `annotations` are `{idempotentHint: true, readOnlyHint: true,
//! destructiveHint: false}`. Also: a `POST` tool defaults the other way, and
//! a non-boolean `idempotent` is refused.

use crate::common;
use common::{McpClient, TestServer, http_kind_registry, signup};
use serde_json::{Value, json};

async fn listed(client: &McpClient, qualified: &str) -> Value {
    let tools = client.tools_list().await.expect("tools/list");
    tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == qualified)
        .unwrap_or_else(|| panic!("{qualified} is listed"))
        .clone()
}

#[tokio::test]
async fn http_get_without_flag_lists_idempotent_and_read_only() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "IdeclAc02").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    for (name, method, expect_idem, expect_ro) in [
        ("fetch_rates", "GET", true, true),
        ("send_invoice", "POST", false, false),
        ("drop_row", "DELETE", false, false),
    ] {
        client
            .tools_call(
                "host.tool_publish",
                json!({"name": name, "kind": "http", "spec": {
                    "method": method,
                    "url": "https://api.example.com/v1/x",
                    "args_schema": {"type": "object"},
                }}),
            )
            .await
            .unwrap_or_else(|e| panic!("publish {name}: {}", e.message));
        let tool = listed(&client, &format!("{ns}.{name}")).await;
        assert_eq!(
            tool["annotations"],
            json!({"idempotentHint": expect_idem, "readOnlyHint": expect_ro, "destructiveHint": !expect_idem}),
            "{name}: {tool}"
        );
    }
}

#[tokio::test]
async fn explicit_flag_overrides_the_http_default_and_must_be_boolean() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "IdeclAc02b").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "upsert_row", "kind": "http", "spec": {
                "method": "POST", "url": "https://api.example.com/v1/rows",
                "args_schema": {"type": "object"}, "idempotent": true,
            }}),
        )
        .await
        .expect("publish POST with idempotent: true");
    let tool = listed(&client, &format!("{ns}.upsert_row")).await;
    // A declared-idempotent POST is not a read.
    assert_eq!(
        tool["annotations"],
        json!({"idempotentHint": true, "readOnlyHint": false, "destructiveHint": false}),
        "{tool}"
    );

    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "bad_flag", "kind": "http", "spec": {
                "method": "GET", "url": "https://api.example.com/v1/x",
                "args_schema": {"type": "object"}, "idempotent": "yes",
            }}),
        )
        .await
        .expect_err("non-boolean idempotent is refused");
    assert!(err.message.contains("idempotent"), "{}", err.message);
}
