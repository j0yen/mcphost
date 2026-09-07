//! PRD-mcphost-tool-test AC1 — Given an authenticated tenant and a valid
//! `python` spec with two example invocations, When it calls
//! `host.spec_test`, Then both invocations run in the sandbox and the
//! response carries per-invocation `ok`, `output`, and `duration_ms`, plus
//! the inferred `args_schema` and `requirements`, and no tool row is
//! persisted.

mod common;
use common::{
    McpClient, TempDataDir, TestServer, extract_structured, poll_spec_test_until_ready,
    python_kind_registry, signup,
};
use serde_json::json;
use std::time::Duration;

fn adder_spec() -> serde_json::Value {
    json!({
        "source": "def main(args):\n    return {\"sum\": args[\"a\"] + args[\"b\"]}\n",
    })
}

#[tokio::test]
async fn two_invocations_run_and_report_schema_and_requirements() {
    let data_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&data_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Spec Test Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let result = poll_spec_test_until_ready(
        &client,
        json!({
            "kind": "python",
            "spec": adder_spec(),
            "invocations": [{"a": 1, "b": 2}, {"a": 10, "b": 20}],
        }),
        Duration::from_secs(30),
    )
    .await;
    let structured = extract_structured(&result);

    assert_eq!(structured["requirements"], json!([]));
    assert!(
        structured["args_schema"].is_object(),
        "{:?}",
        structured["args_schema"]
    );

    let invocations = structured["invocations"].as_array().expect("array");
    assert_eq!(invocations.len(), 2);
    assert_eq!(invocations[0]["ok"], json!(true));
    // `ctx.test_mode = true` (same as `host.tool_test`) makes python's own
    // `call()` wrap the tool's return value under "result" alongside a
    // "schema" field -- proof this reuses the exact same test-mode-aware
    // `Kind::call` path a published tool's dry run does (AC8), not a copy.
    assert_eq!(invocations[0]["output"]["result"]["sum"], json!(3));
    assert!(invocations[0]["duration_ms"].is_number());
    assert_eq!(invocations[1]["ok"], json!(true));
    assert_eq!(invocations[1]["output"]["result"]["sum"], json!(30));

    // No tool row was persisted for this ad hoc spec.
    let listed = client
        .tools_call("host.tool_list", json!({}))
        .await
        .expect("tool_list ok");
    let tools = extract_structured(&listed)["tools"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        tools.is_empty(),
        "host.spec_test must not create a published tool: {tools:?}"
    );
}
