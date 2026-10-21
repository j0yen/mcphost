//! PRD-mcphost-sync-source-registry-and-create AC3 (P0) -- Given a keyed free
//! tenant, When `host.sync.create {source: {kind: "csv_url", ref: "<fixture
//! url>"}, table: "orders"}` is called, Then the response carries `sync_id`,
//! `next_run_unix` within 15 minutes, `host.sync.list` shows one row with
//! `status: "active"`, and the call took < 100 ms.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

const FIXTURE_URL: &str = "https://example.com/orders.csv";

fn create_args(reference: &str) -> Value {
    json!({"source": {"kind": "csv_url", "ref": reference}, "table": "orders"})
}

#[tokio::test]
async fn syreg_ac03() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Syreg AC3").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    assert_eq!(server.state.plans.get("free").expect("free plan").syncs_max, 5);

    let before = mcphost::state::now_unix();
    let created = extract_structured(&client.tools_call("host.sync.create", create_args(FIXTURE_URL)).await.expect("create"));
    let sync_id = created["sync_id"].as_str().expect("sync_id").to_string();
    assert!(!sync_id.is_empty());
    assert_eq!(created["table"], "orders");
    let next = created["next_run_unix"].as_i64().expect("next_run_unix");
    assert!(next > before && next <= before + 15 * 60 + 1, "next_run_unix {next} not within 15 minutes of {before}");

    let listed = extract_structured(&client.tools_call("host.sync.list", json!({})).await.expect("list"));
    let rows = listed["syncs"].as_array().expect("syncs array");
    assert_eq!(rows.len(), 1, "{listed}");
    assert_eq!(rows[0]["sync_id"], json!(sync_id));
    assert_eq!(rows[0]["status"], "active");
    assert_eq!(rows[0]["source"], json!({"kind": "csv_url", "ref": FIXTURE_URL}));

    // The table the sync names exists and answers a query.
    let queried = client.tools_call("host.table.query", json!({"sql": "SELECT COUNT(*) AS n FROM orders"})).await;
    assert!(queried.is_ok(), "{queried:?}");

    // The call itself takes under 100 ms (warm median): a server whose free
    // plan holds enough syncs for every sample, each on its own ref.
    let timed = TestServer::start_with_plans(|catalog| {
        for p in &mut catalog.plans {
            if p.name == "free" {
                p.syncs_max = 100;
            }
        }
    })
    .await;
    let (_ns, key) = signup(&timed.base_url, "Syreg AC3 timing").await;
    let timed_client = McpClient::with_bearer(&timed.base_url, &key);
    let mut n = 0;
    crate::perf_budget!(100, {
        n += 1;
        timed_client
            .tools_call("host.sync.create", create_args(&format!("{FIXTURE_URL}?n={n}")))
            .await
            .expect("timed create");
    });
}
