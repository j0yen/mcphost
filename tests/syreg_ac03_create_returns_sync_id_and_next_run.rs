//! PRD-mcphost-sync-source-registry-and-create AC3 -- Given a keyed free
//! tenant, When `host.sync.create {source: {kind: "csv_url", ref: "<fixture
//! url>"}, table: "orders"}` is called, Then the response carries `sync_id`,
//! `next_run_unix` within 15 minutes, `host.sync.list` shows one row with
//! `status: "active"`, and the call took < 100 ms.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

const FIXTURE_URL: &str = "https://data.example.com/orders.csv";

async fn call(client: &McpClient, tool: &str, args: Value) -> Value {
    extract_structured(&client.tools_call(tool, args).await.unwrap_or_else(|e| panic!("{tool}: {e:?}")))
}

#[tokio::test]
async fn syreg_ac03_create_returns_sync_id_and_a_next_run_within_fifteen_minutes() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "SYREG AC3").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let before = mcphost::state::now_unix();
    let created = call(
        &client,
        "host.sync.create",
        json!({"source": {"kind": "csv_url", "ref": FIXTURE_URL}, "table": "orders"}),
    )
    .await;
    let after = mcphost::state::now_unix();
    let sync_id = created["sync_id"].as_str().expect("sync_id");
    assert!(!sync_id.is_empty(), "{created}");
    assert_eq!(created["table"], "orders", "{created}");
    let next = created["next_run_unix"].as_i64().expect("next_run_unix");
    assert!(next > before && next <= after + 15 * 60, "next_run_unix {next} within 15 minutes of {before}: {created}");

    let listed = call(&client, "host.sync.list", json!({})).await;
    let rows = listed["syncs"].as_array().expect("syncs");
    assert_eq!(rows.len(), 1, "{listed}");
    assert_eq!(rows[0]["sync_id"], created["sync_id"], "{listed}");
    assert_eq!(rows[0]["status"], "active", "{listed}");
    assert_eq!(rows[0]["kind"], "csv_url", "{listed}");

    // The mirror's table exists and answers.
    let count = call(&client, "host.table.count", json!({"table": "orders"})).await;
    assert_eq!(count["count"], 0, "{count}");

    // < 100 ms: the warm median of create + delete (so the free plan's
    // syncs_max never trips), each of them inside the budget.
    crate::perf_budget!(100, {
        let c = call(
            &client,
            "host.sync.create",
            json!({"source": {"kind": "csv_url", "ref": "https://data.example.com/perf.csv"}, "table": "orders_perf"}),
        )
        .await;
        call(&client, "host.sync.delete", json!({"sync_id": c["sync_id"]})).await;
    });
}
