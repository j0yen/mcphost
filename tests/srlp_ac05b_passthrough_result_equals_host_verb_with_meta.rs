//! PRD-mcphost-sandbox-resource-limits-from-plan
//! AC5 (guard) -- Given a python tool that only returns what a host verb
//! returned to it through the bridge, When it is called, Then (1) its
//! top-level structured result equals the host verb's own JSON byte for byte
//! (no `resources_used` inside it) and (2) the call's usage is present in
//! the CallToolResult `_meta.resources_used` with integer `cpu_ms`,
//! `peak_rss_mb` and `wall_ms`.
//!
//! Same shape as `tests/tgraph_ac09_python_sandbox_join_paths.rs`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn passthrough_tool_result_equals_the_host_verb_and_usage_rides_in_meta() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "SRLP AC5b Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({"name": "customers", "columns": {"id": "text", "name": "text"}}),
        )
        .await
        .expect("create customers");
    let customer_ids: Vec<String> = (0..5).map(|i| format!("CUST-{i:04}")).collect();
    let customer_rows: Vec<_> =
        customer_ids.iter().map(|id| json!({"id": id, "name": format!("Customer {id}")})).collect();
    client
        .tools_call("host.table.append", json!({"table": "customers", "rows": customer_rows}))
        .await
        .expect("append customers");

    client
        .tools_call(
            "host.table.create",
            json!({"name": "orders", "columns": {"id": "text", "customer_id": "text", "amount": "real"}}),
        )
        .await
        .expect("create orders");
    let order_rows: Vec<_> = (0..20)
        .map(|i| {
            json!({
                "id": format!("ORD-{i:04}"),
                "customer_id": customer_ids[i % customer_ids.len()],
                "amount": 5.0 + i as f64,
            })
        })
        .collect();
    client.tools_call("host.table.append", json!({"table": "orders", "rows": order_rows})).await.expect("append orders");

    client.tools_call("host.table.describe", json!({"table": "customers"})).await.expect("describe customers");
    client.tools_call("host.table.describe", json!({"table": "orders"})).await.expect("describe orders");

    let mut direct = extract_structured(
        &client
            .tools_call("host.table.join_paths", json!({"from": "orders", "to": "customers"}))
            .await
            .expect("host.table.join_paths"),
    );
    // `data` is the host-added response envelope, not the verb's payload.
    direct.as_object_mut().expect("object").remove("data");

    let source = "import mcphost\ndef main(args):\n    return mcphost.table.join_paths(\"orders\", \"customers\")\n";
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "passthrough_probe", "kind": "python", "spec": {"source": source}}),
        )
        .await
        .expect("publish ok");

    let called = poll_until_ready(&client, &format!("{ns}.passthrough_probe"), json!({}), Duration::from_secs(10))
        .await
        .unwrap_or_else(|e| panic!("pass-through call must succeed: {} {}", e.code, e.message));

    assert_eq!(
        extract_structured(&called),
        direct,
        "a pass-through tool's result must equal the host verb's JSON byte for byte"
    );
    let used = &called["_meta"]["resources_used"];
    for key in ["cpu_ms", "peak_rss_mb", "wall_ms"] {
        assert!(used[key].is_i64(), "_meta.resources_used.{key} must be an integer: {called}");
    }
}
