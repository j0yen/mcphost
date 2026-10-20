//! PRD-mcphost-onboarding-dead-ends-zero AC4 (P0) -- Given a python tool whose
//! call fails `args_invalid` on `n: 4.78` (integer expected), When the error
//! is built, Then `next_call.args.args.n == 4` and `next_call.tool ==
//! "host.tool_call"`.

use crate::common;
use common::{TestServer, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn ndead_ac04_python_args_invalid_next_call_coerces_the_value() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Ndead AC4 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "def main(args):\n    return {'n': args['n']}\n",
        "args_schema": {
            "type": "object",
            "properties": {"n": {"type": "integer"}},
            "required": ["n"],
        },
    });
    client
        .tools_call("host.tool_publish", json!({"name": "count_tool", "kind": "python", "spec": spec}))
        .await
        .expect("publish ok");

    // Both ways a caller reaches the tool: the raw qualified name and the
    // `host.tool_call` wrapper.
    let direct = client
        .tools_call(&format!("{ns}.count_tool"), json!({"n": 4.78}))
        .await
        .expect_err("4.78 is not an integer");
    let wrapped = client
        .tools_call("host.tool_call", json!({"name": "count_tool", "args": {"n": 4.78}}))
        .await
        .expect_err("4.78 is not an integer");

    for err in [direct, wrapped] {
        assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "data: {:?}", err.data);
        let next = &err.data["next_call"];
        assert_eq!(next["tool"], json!("host.tool_call"), "next_call: {next}");
        assert_eq!(next["args"]["args"]["n"], json!(4), "next_call: {next}");
        let name = next["args"]["name"].as_str().unwrap_or_default();
        assert!(name == "count_tool" || name.ends_with(".count_tool"), "next_call: {next}");
    }
}
