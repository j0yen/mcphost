//! PRD-mcphost-tools-list-alias-truth
//! AC2 — Given a client calls `host_tool_run` with a valid published tool,
//! When dispatched, Then it runs as `host.tool.run` and the usage ledger
//! records the canonical name.

use crate::common;
use common::{McpClient, TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn host_tool_run_runs_as_the_canonical_and_the_ledger_records_it() {
    // Real sandboxed python tool, same user-namespace guard as every other
    // python-kind test (host.tool.run is unsupported by the echo kind).
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AliasTruth AC2").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool.publish",
            json!({
                "name": "pinger",
                "kind": "python",
                "spec": {
                    "source": "def main(args):\n    return {\"analysis\": {\"diagnosis\": \"looks fine\"}}\n",
                    "args_schema": {"type": "object"},
                    "outputs": ["diagnosis"],
                },
            }),
        )
        .await
        .expect("publish pinger");

    let tenant = server.state.db.find_tenant_by_namespace(ns).await.unwrap().expect("tenant");
    for name in ["host.tool.run", "host_tool_run", "host_tool_publish"] {
        assert_eq!(
            server.state.db.host_tool_usage_via(tenant.id, name.to_string()).await.unwrap(),
            None,
            "{name} must have no ledger row before the flattened call"
        );
    }

    // The python kind may still be building on the first call.
    let mut flat = None;
    for _ in 0..40 {
        match client.tools_call("host_tool_run", json!({"name": "pinger", "args": {}})).await {
            Ok(r) if extract_structured(&r).get("payload").is_some() => {
                flat = Some(r);
                break;
            }
            _ => tokio::time::sleep(std::time::Duration::from_millis(250)).await,
        }
    }
    let flat = flat.expect("host_tool_run must dispatch and run the published tool");
    let canonical = client
        .tools_call("host.tool.run", json!({"name": "pinger", "args": {}}))
        .await
        .expect("canonical call");
    assert_eq!(
        extract_structured(&flat)["payload"]["diagnosis"],
        extract_structured(&canonical)["payload"]["diagnosis"],
        "flattened call must run the same tool: {flat} vs {canonical}"
    );

    assert_eq!(
        server.state.db.host_tool_usage_via(tenant.id, "host.tool.run".to_string()).await.unwrap().as_deref(),
        Some("direct"),
        "the ledger must record the canonical name"
    );
    assert_eq!(
        server.state.db.host_tool_usage_via(tenant.id, "host_tool_run".to_string()).await.unwrap(),
        None,
        "the ledger must never record the flattened name"
    );
}
