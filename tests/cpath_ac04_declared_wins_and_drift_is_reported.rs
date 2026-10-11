//! PRD-mcphost-chain-publish-time-path-check AC4 (P0) -- Given a predecessor
//! declaring `outputs: [rows]` whose 2 runs returned `{items}`, When a chain
//! maps `$.prev.result.rows`, Then publish succeeds, `prev_fields_source` is
//! `declared`, and `observed_drift` is `["rows"]`.
//!
//! The two runs are recorded through `Db::merge_observed_outputs` -- the one
//! call a finished run makes -- because a declaring kind with a runnable
//! body (python) needs a userns sandbox this suite does not assume.

use crate::common;
use common::{McpClient, TestServer, chain_and_http_kind_registry, extract_structured, publish, signup};
use serde_json::json;

async fn setup(name: &str) -> (TestServer, McpClient, String) {
    let server = TestServer::start_with_kinds(chain_and_http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, name).await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    (server, client, ns)
}

#[tokio::test]
async fn declared_outputs_are_trusted_and_unobserved_ones_are_reported_as_drift() {
    let (server, client, ns) = setup("Cpath AC4 Tenant").await;
    publish(
        &client,
        "src",
        "http",
        json!({
            "method": "GET",
            "url": "http://127.0.0.1:1/x",
            "args_schema": {"type": "object"},
            "outputs": ["rows"],
        }),
    )
    .await;
    publish(&client, "sink", "echo", json!({"schema": {"type": "object"}})).await;
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("find").expect("tenant");
    for _ in 0..2 {
        let shape = [("items".to_string(), "array".to_string())].into_iter().collect();
        server.state.db.merge_observed_outputs(tenant.id, "src".to_string(), shape).await.expect("merge");
    }

    let spec = json!({"steps": [
        {"tool": "src", "args": {}},
        {"tool": "sink", "args": {"rows": "$.prev.result.rows"}},
    ]});
    let published = extract_structured(
        &client
            .tools_call("host.tool_publish", json!({"name": "pipeline", "kind": "chain", "spec": spec}))
            .await
            .expect("a declared output must publish even when no run returned it"),
    );

    assert_eq!(published["prev_fields_source"], json!("declared"), "{published}");
    assert_eq!(published["observed_drift"], json!(["rows"]), "{published}");
    assert_eq!(published["runs_sampled"], json!(2), "{published}");
    assert_eq!(published["verdict"], json!("pass"), "{published}");
}
