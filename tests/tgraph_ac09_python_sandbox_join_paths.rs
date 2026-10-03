//! PRD-mcphost-table-concept-graph
//! AC9 — Given the python sandbox, When a tool calls
//! `mcphost.table.join_paths("orders", "customers")`, Then it receives the
//! same object the tool returns.
//!
//! Same shape as `tests/tables_ac05_python_sandbox_table_access.rs` (the
//! `mcphost.table.create`/`.append`/`.query` proof for
//! PRD-mcphost-tenant-tables): a real python-kind tool, run through the
//! real sandbox, calling `mcphost.table.join_paths` over the
//! `TableSidecarBridge` request/response channel, compared against calling
//! `host.table.join_paths` directly as an ordinary session tool.

use crate::common;
use common::{McpClient, TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn mcphost_table_join_paths_from_sandboxed_tool_matches_the_host_tool() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "TGraph AC9 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({"name": "customers", "columns": {"id": "text", "name": "text"}}),
        )
        .await
        .expect("create customers");
    let customer_ids: Vec<String> = (0..20).map(|i| format!("CUST-{i:04}")).collect();
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
    let order_rows: Vec<_> = (0..100)
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

    let direct = extract_structured(
        &client
            .tools_call("host.table.join_paths", json!({"from": "orders", "to": "customers"}))
            .await
            .expect("host.table.join_paths"),
    );

    let source = r#"import mcphost
def main(args):
    result = mcphost.table.join_paths("orders", "customers")
    return result
"#;
    let spec = json!({"source": source});
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "join_path_probe", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let qualified = format!("{ns}.join_path_probe");
    let called = poll_until_ready(&client, &qualified, json!({}), Duration::from_secs(10))
        .await
        .unwrap_or_else(|e| panic!("sandboxed join_paths call must succeed: {} {}", e.code, e.message));
    let from_sandbox = extract_structured(&called);

    assert_eq!(
        from_sandbox, direct,
        "mcphost.table.join_paths from the sandbox must return the same object host.table.join_paths does"
    );
}
