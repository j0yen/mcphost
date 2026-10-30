//! PRD-mcphost-chain-near-miss-mapping-refused AC6 (P0) -- Given a published
//! chain whose step 2 tool `transform_data` raises `TypeError` at call time,
//! When `host.tool_call` fails, Then the error message is `step 2
//! ('transform_data'): ` followed by the child's message, `data.failed_step`
//! is 2, `data.step_tool` is `transform_data`, and the error code is the
//! child's; a step-1 failure reads `step 1 ('fetch_data'): ...`.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, chain_and_python_kind_registry, publish, signup};
use mcphost::sandbox;
use serde_json::json;

const FETCH: &str = "def main(args):\n    return {\"rows\": [{\"id\": 1, \"value\": 2}]}\n";
const BROKEN_FETCH: &str = "def main(args):\n    raise ValueError(\"upstream is down\")\n";
const TRANSFORM: &str = "def main(args):\n    return {\"total\": sum(r[\"value\"] for r in args[\"rows\"])}\n";

#[tokio::test]
async fn a_step_failure_message_begins_with_the_step_and_its_tool() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs = TempDataDir::new();
    let server = TestServer::start_with_kinds(chain_and_python_kind_registry(&envs.0)).await;
    let (_ns, key) = signup(&server.base_url, "Chainmap AC6 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    publish(&client, "fetch_data", "python", json!({"source": FETCH})).await;
    publish(&client, "broken_fetch", "python", json!({"source": BROKEN_FETCH})).await;
    // `transform_data` raises TypeError when `rows` is not a list of objects.
    publish(&client, "transform_data", "python", json!({"source": TRANSFORM})).await;

    // Step 2 receives a literal string, so `r["value"]` raises TypeError.
    let spec = json!({"steps": [
        {"tool": "fetch_data", "args": {}},
        {"tool": "transform_data", "args": {"rows": "not-a-list"}},
    ]});
    publish(&client, "ok_shape", "chain", spec).await;
    let child = client
        .tools_call("host.tool_call", json!({"name": "transform_data", "args": {"rows": "not-a-list"}}))
        .await
        .expect_err("the child raises on its own");
    assert!(child.message.contains("TypeError"), "{child:?}");

    let err = client
        .tools_call("host.tool_call", json!({"name": "ok_shape", "args": {}}))
        .await
        .expect_err("step 2 raises");
    assert!(err.message.starts_with("step 2 ('transform_data'): "), "{err:?}");
    assert!(err.message.contains("TypeError"), "{err:?}");
    assert_eq!(err.data["failed_step"], json!(2), "{:?}", err.data);
    assert_eq!(err.data["step_tool"], json!("transform_data"), "{:?}", err.data);
    assert_eq!(err.error_code, child.error_code, "the code stays the child's: {err:?} vs {child:?}");

    let spec = json!({"steps": [
        {"tool": "broken_fetch", "args": {}},
        {"tool": "transform_data", "args": {"rows": "$.prev.result.rows"}},
    ]});
    publish(&client, "first_fails", "chain", spec).await;
    let first = client
        .tools_call("host.tool_call", json!({"name": "first_fails", "args": {}}))
        .await
        .expect_err("step 1 raises");
    assert!(first.message.starts_with("step 1 ('broken_fetch'): "), "{first:?}");
    assert!(first.message.contains("upstream is down"), "{first:?}");
    assert_eq!(first.data["failed_step"], json!(1), "{:?}", first.data);
    assert_eq!(first.data["step_tool"], json!("broken_fetch"), "{:?}", first.data);
}
