//! PRD-mcphost-onboarding-dead-ends-zero
//! AC4 — Given a python tool whose call fails `args_invalid` on `n: 4.78`
//! (integer expected), When the error is built, Then `next_call.args.args.n
//! == 4` and `next_call.tool == "host.tool_call"`.

use crate::common;
use common::{TestServer, python_kind_registry, signup};
use mcphost::errors::AppError;
use mcphost::kinds::describe_args_error;
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn ndead_ac04() {
    // The error as the python kind builds it (`describe_args_error` ->
    // `Structured args_invalid`), tagged with the tool the way the call path
    // does.
    let schema = json!({"type": "object", "properties": {"n": {"type": "integer"}}, "required": ["n"]});
    let validator = jsonschema::validator_for(&schema).expect("schema");
    let args = json!({"n": 4.78});
    let violation = validator.validate(&args).expect_err("4.78 is not an integer");
    let args_err = describe_args_error(&violation);
    let built = AppError::Structured { code: "args_invalid", message: args_err.message(), data: args_err.data() }
        .with_tool("t_abc.rate");
    let data = built.into_error_data().data.expect("data");
    assert_eq!(data["next_call"]["tool"], "host.tool_call", "{data}");
    assert_eq!(data["next_call"]["args"]["name"], "t_abc.rate", "{data}");
    assert_eq!(data["next_call"]["args"]["args"]["n"], 4, "{data}");

    // And end to end through a published python tool.
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC4 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);
    let spec = json!({"source": "def main(args):\n    return {'n': args['n']}\n", "args_schema": schema});
    client
        .tools_call("host.tool_publish", json!({"name": "rate", "kind": "python", "spec": spec}))
        .await
        .expect("publish ok");
    let name = format!("{ns}.rate");
    let err = client
        .tools_call(&name, json!({"n": 4.78}))
        .await
        .expect_err("4.78 is not an integer");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{}", err.data);
    assert_eq!(err.data["next_call"]["tool"], "host.tool_call", "{}", err.data);
    assert_eq!(err.data["next_call"]["args"]["args"]["n"], 4, "{}", err.data);
    // The call it names, executed as given, succeeds.
    let next = &err.data["next_call"];
    let ok = client
        .tools_call(next["tool"].as_str().unwrap(), next["args"].clone())
        .await;
    assert!(ok.is_ok(), "next_call as given must succeed: {ok:?}");
}
