//! PRD-mcphost-dry-run-side-effects
//! AC4 — Given `host.tool_run(name, args, test: true)` on a python tool
//! that writes state, When it completes, Then stdout/stderr/exit code are
//! returned as today and `dry_run.writes` lists the state write while
//! `host.state.get` returns the prior value.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, extract_structured, publish, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn tool_run_test_rolls_back_a_state_write_and_reports_it() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Dry Run AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // A prior real value -- what `host.state.get` must still read back
    // after the test run below.
    client
        .tools_call("host.state.set", json!({"key": "count", "value": 1}))
        .await
        .expect("state set ok");

    let source = r#"import mcphost
def main(args):
    mcphost.state.set("count", 2)
    return {"ok": True}
"#;
    publish(&client, "writer", "python", json!({"source": source})).await;

    let result = client
        .tools_call(
            "host.tool_run",
            json!({"name": "writer", "args": {}, "test": true}),
        )
        .await
        .expect("tool_run ok");
    let structured = extract_structured(&result);

    // stdout/stderr/exit_code returned as today.
    assert_eq!(structured["exit_code"], json!(0), "{structured:?}");
    assert!(structured["stdout"].is_string(), "{structured:?}");
    assert!(structured["stderr"].is_string(), "{structured:?}");
    assert_eq!(structured["payload"]["ok"], json!(true), "{structured:?}");

    let writes = structured["dry_run"]["writes"].as_array().expect("writes array");
    assert_eq!(writes.len(), 1, "{writes:?}");
    assert_eq!(writes[0]["store"], json!("state"), "{writes:?}");
    assert_eq!(writes[0]["op"], json!("set"), "{writes:?}");
    assert_eq!(writes[0]["key"], json!("count"), "{writes:?}");
    assert_eq!(structured["dry_run"]["rolled_back"], json!(true), "{structured:?}");
    assert_eq!(structured["dry_run"]["delivered"], json!(false), "{structured:?}");

    let got = client
        .tools_call("host.state.get", json!({"key": "count"}))
        .await
        .expect("state get ok");
    let got = extract_structured(&got);
    assert_eq!(
        got["value"],
        json!(1),
        "a dry run must not persist the state write: {got:?}"
    );
}
