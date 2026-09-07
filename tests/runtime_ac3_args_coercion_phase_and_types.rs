//! AC3 (P0) — Given a call whose argument fails schema coercion, When it
//! runs, Then the error's phase is `args_coercion` and it names the
//! argument and both types.
//!
//! Scoped decision (see `kinds::python::describe_args_error`'s doc
//! comment): mcphost has no argument-coercion step separate from JSON
//! Schema validation at call time -- args reach `main(args)` exactly as
//! the caller sent them. That validation gate IS the `args_coercion` phase
//! this AC names; there is no other place in the host where an argument's
//! type is checked before the call is dispatched.

mod common;
use common::{TestServer, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn wrong_typed_argument_names_argument_and_both_types() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC3 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let source = "def main(args):\n    return {'top_k': args['top_k']}\n";
    let spec = json!({
        "source": source,
        "args_schema": {
            "type": "object",
            "properties": {"top_k": {"type": "integer"}},
            "required": ["top_k"],
        },
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "typed_tool", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    // No env-build race to poll for here -- schema validation happens
    // before the sandbox is even consulted, so this fails immediately
    // regardless of build state.
    let err = client
        .tools_call(
            &format!("{ns}.typed_tool"),
            json!({"top_k": "five"}),
        )
        .await
        .expect_err("a string is not a valid 'integer' argument");

    assert_eq!(err.error_code.as_deref(), Some("args_invalid"));
    assert_eq!(
        err.data["phase"].as_str(),
        Some("args_coercion"),
        "data: {:?}",
        err.data
    );
    assert_eq!(err.data["argument"].as_str(), Some("top_k"));
    assert_eq!(err.data["expected_type"].as_str(), Some("integer"));
    assert_eq!(err.data["actual_type"].as_str(), Some("string"));
}
