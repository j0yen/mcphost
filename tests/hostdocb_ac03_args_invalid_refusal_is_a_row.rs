//! PRD-mcphost-host-doctor-tool-b
//! AC3 -- Given a call refused with `args_invalid` before execution, When
//! `host.doctor` runs, Then that refusal is a row with `outcome: refused` in
//! the `args_invalid` group and the tenant's metered call count did not change.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn a_pre_execution_args_invalid_refusal_is_a_refused_row_in_the_args_invalid_group_and_unmetered() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "hostdocb ac3").await;
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("lookup").expect("tenant");
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "count_rows",
                "kind": "echo",
                "spec": {"schema": {
                    "type": "object",
                    "properties": {"limit": {"type": "integer"}},
                    "required": ["limit"]
                }}
            }),
        )
        .await
        .expect("publish");

    let metered = || async { server.state.db.count_calls_since(tenant.id, 0, true).await.expect("count") };
    let before = metered().await;
    let err = client
        .tools_call("host.tool_call", json!({"name": "count_rows", "args": {"limit": 4.78}}))
        .await
        .expect_err("refused");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{} {}", err.code, err.message);
    assert_eq!(metered().await, before, "a refusal must not be metered");

    let out = extract_structured(&client.tools_call("host.doctor", json!({})).await.expect("doctor"));
    let groups = out["groups"].as_array().expect("groups");
    let g = groups.iter().find(|g| g["code"] == json!("args_invalid")).unwrap_or_else(|| panic!("no args_invalid group: {out}"));
    assert_eq!(g["count"], json!(1), "{out}");
    assert_eq!(g["outcomes"], json!({"refused": 1}), "{out}");
    assert_eq!(g["tools"], json!(["count_rows"]), "{out}");
    let next = g["next"].as_str().expect("next");
    assert!(next.contains("`count_rows`") && !next.contains('{'), "{next}");
    assert_eq!(out["window"]["failed"], json!(1), "{out}");
}
