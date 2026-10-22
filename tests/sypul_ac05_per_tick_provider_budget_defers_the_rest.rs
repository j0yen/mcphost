//! PRD-mcphost-sync-pull-with-change-detection AC5 -- Given a free tenant with
//! 25 due syncs, When one tick runs, Then at most 20 provider calls are made
//! and the other 5 carry `status: deferred` and run on the next tick.

use crate::common::sypul::{Fixture, orders_csv};
use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

const SYNCS: usize = 25;

async fn call(client: &McpClient, tool: &str, args: Value) -> Value {
    extract_structured(&client.tools_call(tool, args).await.unwrap_or_else(|e| panic!("{tool}: {e:?}")))
}

async fn last_statuses(client: &McpClient, ids: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for id in ids {
        let got = call(client, "host.sync.get", json!({"sync_id": id})).await;
        out.push(got["last_run"]["status"].as_str().unwrap_or("none").to_string());
    }
    out
}

#[tokio::test]
async fn sypul_ac05_a_free_tenant_spends_at_most_20_provider_calls_per_tick() {
    let server = TestServer::start_with_plans(|catalog| {
        for p in &mut catalog.plans {
            if p.name == "free" {
                p.syncs_max = 30;
                p.table_tables_max = 30;
                assert_eq!(p.sync_provider_calls_per_tick, 20, "the free plan's per-tick budget");
            }
        }
    })
    .await;
    let (_ns, key) = signup(&server.base_url, "SYPUL AC5").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let fixture = Fixture::start(&orders_csv(3), false).await;

    let mut ids = Vec::new();
    for i in 0..SYNCS {
        let created = call(
            &client,
            "host.sync.create",
            json!({"source": {"kind": "csv_url", "ref": fixture.url(&format!("s{i}.csv"))}, "table": format!("t{i}")}),
        )
        .await;
        ids.push(created["sync_id"].as_str().expect("sync_id").to_string());
    }
    let past = mcphost::state::now_unix() - 60;
    for id in &ids {
        mcphost::sync::set_next_run_for_test(&server.state, id, past).await.expect("make due");
    }

    mcphost::triggers::tick_once(&server.state).await.expect("first tick");
    assert_eq!(fixture.hits(), 20, "at most 20 provider calls in one tick");
    let statuses = last_statuses(&client, &ids).await;
    assert_eq!(statuses.iter().filter(|s| *s == "ok").count(), 20, "{statuses:?}");
    assert_eq!(statuses.iter().filter(|s| *s == "deferred").count(), 5, "{statuses:?}");

    mcphost::triggers::tick_once(&server.state).await.expect("second tick");
    assert_eq!(fixture.hits(), 25, "the deferred five ran on the next tick");
    let statuses = last_statuses(&client, &ids).await;
    assert!(statuses.iter().all(|s| s == "ok"), "{statuses:?}");
}
