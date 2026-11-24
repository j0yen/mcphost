//! PRD-mcphost-response-size-budget-explicit-v2
//! AC4 (P0) -- Given a python tool that returns a 2 MiB line, When called
//! synchronously, Then the existing `tool_output_too_large` answers with
//! `limit_bytes` equal to `limits.output_bytes_max` and no `truncated` key;
//! Given a non-paged verb under the wall, Then its response is whole.

use crate::common;
use common::{McpClient, TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::{Value, json};
use std::time::Duration;

#[tokio::test]
async fn rszb_ac04_a_two_mib_python_line_is_refused_by_the_wall_not_cut() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Rszb AC4 Wall").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "def main(args):\n    return {\"line\": \"x\" * args.get(\"n\", 0)}\n",
        "args_schema": {"type": "object"},
    });
    client
        .tools_call("host.tool_publish", json!({"name": "wideline", "kind": "python", "spec": spec}))
        .await
        .expect("publish");
    let qualified = format!("{ns}.wideline");

    // Under the wall the line comes back whole.
    let ok = poll_until_ready(&client, &qualified, json!({"n": 100 * 1024}), Duration::from_secs(20))
        .await
        .expect("a 100 KiB line succeeds");
    assert_eq!(extract_structured(&ok)["line"].as_str().map(str::len), Some(100 * 1024));
    assert!(ok["_meta"].get("mcphost").is_none_or(Value::is_null), "{ok:?}");

    // 2 MiB is refused by the existing wall, naming the wall's own number.
    let err = client
        .tools_call(&qualified, json!({"n": 2 * 1024 * 1024}))
        .await
        .expect_err("a 2 MiB line is refused, not cut");
    assert_eq!(err.error_code.as_deref(), Some("tool_output_too_large"), "{err:?}");
    let quickstart = extract_structured(&client.tools_call("host.quickstart", json!({"kind": "echo"})).await.expect("quickstart"));
    assert_eq!(err.data["limit_bytes"], quickstart["limits"]["output_bytes_max"], "{err:?}");
    assert_eq!(err.data["limit_bytes"], json!(1024 * 1024), "{err:?}");
    assert!(err.data["actual_bytes"].as_u64().expect("actual_bytes") >= 2 * 1024 * 1024, "{err:?}");
    assert!(err.data.get("truncated").is_none(), "nothing was cut, so no marker: {err:?}");
    assert!(!err.message.contains("continue_with"), "{err:?}");
}

#[tokio::test]
async fn rszb_ac04_a_non_paged_verb_over_the_plan_budget_is_returned_whole() {
    const BUDGET: i64 = 4096;
    let server = TestServer::start_with_plans(|catalog| {
        for plan in &mut catalog.plans {
            plan.response_budget_bytes = BUDGET;
            plan.tools_max = 200;
        }
    })
    .await;
    let (_ns, key) = signup(&server.base_url, "Rszb AC4 Whole").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    assert!(!mcphost::paged::PAGED_VERBS.contains(&"host.tool.list"), "the fixture verb must be non-paged");
    for i in 0..40 {
        client
            .tools_call(
                "host.tool.publish",
                json!({"name": format!("whole_{i:02}"), "kind": "echo", "spec": {"schema": {"type": "object"}}}),
            )
            .await
            .expect("publish");
    }

    let result = client.tools_call("host.tool.list", json!({})).await.expect("list");
    let body = extract_structured(&result);
    let tools = body["tools"].as_array().expect("tools");
    assert_eq!(tools.len(), 40, "every row is returned: {body}");
    let size = serde_json::to_vec(tools).expect("serialize").len();
    assert!(size > BUDGET as usize, "the response ({size} bytes) must pass the plan budget for this to prove anything");
    assert!(result["_meta"].get("mcphost").is_none_or(Value::is_null), "no marker on a whole response: {result:?}");
    assert!(body.get("next_cursor").is_none(), "{body}");
}
