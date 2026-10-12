//! PRD-mcphost-destructive-verb-confirmation-token
//! AC7 -- Given plan `free` with `confirm_rows_min: 100` and a plan override
//! setting it to 0, When a 5 000-row delete is called on the overridden
//! tenant, Then it runs without a token and
//! `host.quickstart.limits.plan.confirm_rows_min == 0`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

async fn seed_jobs(client: &McpClient, total: usize) {
    client
        .tools_call(
            "host.state.table_create",
            json!({"name": "jobs", "schema": {"n": "integer", "status": "text"}}),
        )
        .await
        .expect("table_create");
    let mut n = 0;
    while n < total {
        let end = (n + 200).min(total);
        let rows: Vec<Value> = (n..end).map(|i| json!({"n": i, "status": "done"})).collect();
        client
            .tools_call("host.state.insert", json!({"table": "jobs", "rows": rows}))
            .await
            .expect("insert batch");
        n = end;
    }
}

async fn plan_confirm_rows_min(client: &McpClient) -> Value {
    let quickstart = extract_structured(&client.tools_call("host.quickstart", json!({"kind": "echo"})).await.expect("quickstart"));
    quickstart["limits"]["plan"]["confirm_rows_min"].clone()
}

#[tokio::test]
async fn a_plan_with_confirm_rows_min_zero_runs_a_big_delete_without_a_token() {
    // The shipped default is 100 and shows in the quickstart limits.
    let default_server = TestServer::start().await;
    let (_ns, key) = signup(&default_server.base_url, "Cnfm AC7 Default").await;
    let default_client = McpClient::with_bearer(&default_server.base_url, &key);
    assert_eq!(plan_confirm_rows_min(&default_client).await, json!(100));

    let server = TestServer::start_with_plans(|catalog| {
        for p in &mut catalog.plans {
            if p.name == "free" {
                p.confirm_rows_min = 0;
            }
        }
    })
    .await;
    let (_ns, key) = signup(&server.base_url, "Cnfm AC7 Override").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    assert_eq!(plan_confirm_rows_min(&client).await, json!(0));

    seed_jobs(&client, 5_000).await;
    let deleted = extract_structured(
        &client
            .tools_call("host.state.delete_rows", json!({"table": "jobs", "where": {}}))
            .await
            .expect("a 5 000-row delete runs without a token when the gate is off"),
    );
    assert_eq!(deleted["deleted"], json!(5_000), "{deleted}");

    // And a dry run mints nothing on that plan.
    let dry = extract_structured(
        &client
            .tools_call("host.state.delete_rows", json!({"table": "jobs", "where": {}, "dry_run": true}))
            .await
            .expect("dry run"),
    );
    assert!(dry.get("confirm").is_none(), "{dry}");
}
