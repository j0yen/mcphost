//! PRD-mcphost-chart-in-a-minute
//! AC11 — Given a `python` tool calling `mcphost.table.chart(sql)`, When it
//! runs in the sandbox, Then it receives the same `chart.v1` object the
//! tool returns.

use crate::common;
use common::{McpClient, TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

use crate::chart_fixture;

#[tokio::test]
async fn python_bridge_chart_call_returns_chart_v1() {
    // Same skip-in-CI, fail-loudly-elsewhere convention every other
    // sandbox-dependent test in this crate uses (see
    // `tests/tables_ac05_python_sandbox_table_access.rs`).
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Chart AC11 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    chart_fixture::seed_expenses(&client).await;

    let source = r#"import mcphost
def main(args):
    return mcphost.table.chart(
        sql="SELECT category, SUM(amount) AS total FROM expenses GROUP BY category",
    )
"#;
    let spec = json!({"source": source});
    client
        .tools_call("host.tool_publish", json!({"name": "chart_tool", "kind": "python", "spec": spec}))
        .await
        .expect("publish ok");

    let qualified = format!("{ns}.chart_tool");
    let called = poll_until_ready(&client, &qualified, json!({}), Duration::from_secs(10))
        .await
        .unwrap_or_else(|e| panic!("sandboxed chart call must succeed: {} {}", e.code, e.message));
    let via_sandbox = extract_structured(&called);

    let direct_result = client
        .tools_call(
            "host.table.chart",
            json!({"sql": "SELECT category, SUM(amount) AS total FROM expenses GROUP BY category"}),
        )
        .await
        .expect("host.table.chart");
    let direct = extract_structured(&direct_result);

    assert_eq!(via_sandbox["schema"], json!("chart.v1"), "sandbox result: {via_sandbox}");
    assert_eq!(via_sandbox["row_count"], direct["row_count"], "sandbox: {via_sandbox} direct: {direct}");
    assert_eq!(
        via_sandbox["recommendation"]["mark"], direct["recommendation"]["mark"],
        "sandbox: {via_sandbox} direct: {direct}"
    );
    assert_eq!(
        via_sandbox["caption"]["headline"], direct["caption"]["headline"],
        "sandbox: {via_sandbox} direct: {direct}"
    );
    assert_eq!(via_sandbox["vega_lite"]["mark"], direct["vega_lite"]["mark"], "sandbox: {via_sandbox}");
}
