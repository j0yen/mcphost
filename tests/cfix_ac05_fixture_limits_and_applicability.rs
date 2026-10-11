//! PRD-mcphost-chain-dry-run AC5 (P0) -- Given fixtures totalling 300 KiB,
//! When `host.tool_test` runs, Then `fixtures_too_large` with
//! `data.limit_bytes` 262144; Given a python tool with `fixtures`, Then
//! `fixtures_not_applicable`.

use crate::common;
use common::{McpClient, TestServer, chain_and_python_kind_registry, chain_kind_registry, publish, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn oversized_fixtures_are_refused_with_the_limit() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Cfix AC5 Size Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    publish(&client, "hop", "echo", json!({"schema": {"type": "object"}})).await;
    let spec = json!({"steps": [{"tool": "hop", "args": {"x": 1}}]});
    publish(&client, "pipeline", "chain", spec).await;

    let big = "x".repeat(300 * 1024);
    let err = client
        .tools_call(
            "host.tool_test",
            json!({"name": "pipeline", "args": {}, "fixtures": {"1": {"blob": big}}}),
        )
        .await
        .expect_err("300 KiB of fixtures must be refused");
    assert_eq!(err.error_code.as_deref(), Some("fixtures_too_large"), "{err:?}");
    assert_eq!(err.data["limit_bytes"], json!(262144), "{err:?}");
}

#[tokio::test]
async fn fixtures_on_a_non_chain_tool_are_not_applicable() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Cfix AC5 Kind Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    publish(&client, "plain", "echo", json!({"schema": {"type": "object"}})).await;

    let err = client
        .tools_call(
            "host.tool_test",
            json!({"name": "plain", "args": {}, "fixtures": {"1": {"a": 1}}}),
        )
        .await
        .expect_err("fixtures on a non-chain tool must be refused");
    assert_eq!(err.error_code.as_deref(), Some("fixtures_not_applicable"), "{err:?}");
}

#[tokio::test]
async fn fixtures_on_a_python_tool_are_not_applicable() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(chain_and_python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Cfix AC5 Python Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let source = "def main(args):\n    return {\"ok\": True}\n";
    client
        .tools_call("host.tool_publish", json!({"name": "pytool", "kind": "python", "spec": {"source": source}}))
        .await
        .expect("publish python tool");

    let err = client
        .tools_call(
            "host.tool_test",
            json!({"name": "pytool", "args": {}, "fixtures": {"1": {"a": 1}}}),
        )
        .await
        .expect_err("fixtures on a python tool must be refused");
    assert_eq!(err.error_code.as_deref(), Some("fixtures_not_applicable"), "{err:?}");
}
