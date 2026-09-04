//! PRD-mcphost-tool-infer AC1/AC2/AC3 (P0) — Given a `python` tool with no
//! `args_schema`, source reading `args["city"]` (required) and
//! `args.get("units")` (optional), When it is published, Then `tools/list`
//! shows `city` as required and `units` as optional, and a call supplying
//! only `city` succeeds.

mod common;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn required_and_optional_keys_are_inferred_and_a_partial_call_succeeds() {
    // Requirement 8/9: this test builds and runs a real python-kind tool
    // via the sandbox, which needs unprivileged user namespaces. Not
    // guaranteed on GitHub's hosted runners, so skip cleanly in CI (and
    // fail loudly, not skip, anywhere else -- see
    // require_user_namespaces_or_ci_skip's doc comment) rather than fail
    // with "the tool's environment failed to build" -- same pattern as the
    // sandbox-dependent unit tests in src/kinds/python.rs and
    // src/sandbox.rs, and as tests/ac17_kind_conformance.rs.
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("skipped: no user namespaces (CI)");
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC1-3 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "def main(args):\n    return {\"city\": args[\"city\"], \"units\": args.get(\"units\", \"metric\")}\n",
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "weather", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish without args_schema must succeed via inference");

    let listed = client.tools_list().await.expect("tools/list ok");
    let tool = listed["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|t| t["name"] == format!("{ns}.weather"))
        .expect("published tool listed")
        .clone();
    let schema = &tool["inputSchema"];

    // AC1: city is required.
    let required: Vec<&str> = schema["required"]
        .as_array()
        .expect("required array present")
        .iter()
        .map(|v| v.as_str().expect("string"))
        .collect();
    assert_eq!(required, vec!["city"], "only 'city' must be required");
    assert!(
        schema["properties"]["city"].is_object(),
        "city must be a property"
    );

    // AC2: units is a property but not required.
    assert!(
        schema["properties"]["units"].is_object(),
        "units must be a property"
    );
    assert!(
        !required.contains(&"units"),
        "units must be optional, not required"
    );

    // AC3: a call supplying only the required key succeeds.
    let result = poll_until_ready(
        &client,
        &format!("{ns}.weather"),
        json!({"city": "Boston"}),
        Duration::from_secs(60),
    )
    .await
    .unwrap_or_else(|e| panic!("call with only the required key must succeed: {} {}", e.code, e.message));
    let structured = extract_structured(&result);
    assert_eq!(structured["city"], "Boston");
    assert_eq!(structured["units"], "metric");
}
