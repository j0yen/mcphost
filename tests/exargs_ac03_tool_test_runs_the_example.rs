//! PRD-mcphost-tool-test-examples-from-schema AC3 — a tool whose schema is
//! `{"properties": {"n": {"type": "integer", "default": 2}}}`, tested with no
//! `args`, runs with `{"n": 2}` and reports `example_args: {"n": 2}` and
//! `untyped: []`; the same call with `args: {"n": 5}` carries no
//! `example_args`. The echo kind returns exactly the arguments it ran with,
//! which proves what the host passed; the python source below proves the
//! schema is what python inference produces for such a tool.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::kinds::KindRegistry;
use mcphost::kinds::infer::infer_python_args_schema;
use serde_json::json;

#[test]
fn python_inference_yields_the_schema_the_example_is_generated_from() {
    let schema = infer_python_args_schema("def main(args):\n    return args.get('n', 2)\n").expect("inferable");
    assert_eq!(schema["properties"]["n"], json!({"type": "integer", "default": 2}));
}

#[tokio::test]
async fn tool_test_without_args_runs_the_example_and_reports_it() {
    let server = TestServer::start_with_kinds(KindRegistry::with_builtin()).await;
    let (_ns, key) = signup(&server.base_url, "Exargs AC3").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "counter",
                "kind": "echo",
                "spec": {"schema": {"properties": {"n": {"type": "integer", "default": 2}}}},
            }),
        )
        .await
        .expect("publish counter");

    let out = extract_structured(&client.tools_call("host.tool_test", json!({"name": "counter"})).await.expect("test without args"));
    assert_eq!(out["n"], json!(2), "the tool ran with the generated args: {out}");
    assert_eq!(out["example_args"], json!({"n": 2}));
    assert_eq!(out["untyped"], json!([]));
    assert!(out.get("schema_hint").is_none(), "no hint when nothing is untyped");

    let given = extract_structured(
        &client.tools_call("host.tool_test", json!({"name": "counter", "args": {"n": 5}})).await.expect("test with args"),
    );
    assert_eq!(given["n"], json!(5));
    assert!(given.get("example_args").is_none(), "explicit args carry no example_args: {given}");
}
