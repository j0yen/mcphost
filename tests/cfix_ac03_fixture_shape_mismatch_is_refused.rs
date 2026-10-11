//! PRD-mcphost-chain-dry-run AC3 (P0) -- Given step 2's tool declares
//! `outputs: [items]` and a fixture `{"rows": []}` for step 2, When
//! `host.tool_test` runs, Then it is refused with `fixture_shape_mismatch`,
//! `data.expected` `["items"]`, `data.given` `["rows"]`.

use crate::common;
use common::{McpClient, TestServer, chain_and_http_kind_registry, publish, signup};
use serde_json::json;

#[tokio::test]
async fn a_fixture_missing_every_declared_output_is_refused_with_both_key_sets() {
    let server = TestServer::start_with_kinds(chain_and_http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Cfix AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    publish(&client, "head", "echo", json!({"schema": {"type": "object"}})).await;
    publish(
        &client,
        "declared",
        "http",
        json!({"method": "GET", "url": "http://127.0.0.1:1/x", "args_schema": {"type": "object"},
               "outputs": ["items"]}),
    )
    .await;
    publish(&client, "sink", "echo", json!({"schema": {"type": "object"}})).await;
    let spec = json!({"steps": [
        {"tool": "head", "args": {"seed": 1}},
        {"tool": "declared", "args": {}},
        {"tool": "sink", "args": {"rows": "$.prev.result.items"}},
    ]});
    publish(&client, "pipeline", "chain", spec).await;

    let err = client
        .tools_call(
            "host.tool_test",
            json!({"name": "pipeline", "args": {}, "fixtures": {"2": {"rows": []}}}),
        )
        .await
        .expect_err("a mismatched fixture must be refused");
    assert_eq!(err.error_code.as_deref(), Some("fixture_shape_mismatch"), "{err:?}");
    assert_eq!(err.data["expected"], json!(["items"]), "{err:?}");
    assert_eq!(err.data["given"], json!(["rows"]), "{err:?}");
    assert_eq!(err.data["step"], json!(2), "{err:?}");

    // A fixture that carries a declared key is accepted and the chain passes.
    let ok = common::extract_structured(
        &client
            .tools_call(
                "host.tool_test",
                json!({"name": "pipeline", "args": {}, "fixtures": {"2": {"items": [1]}}}),
            )
            .await
            .expect("a matching fixture is accepted"),
    );
    assert_eq!(ok["verdict"], json!("pass"), "{ok}");
    assert_eq!(ok["steps"][1]["source"], json!("fixture"), "{ok}");
}
