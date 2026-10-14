//! PRD-mcphost-scheduled-job-receipts AC5 (P0) — Given a free tenant with
//! receipts 8 days old, When the sweeper runs, Then those rows are gone,
//! `summary.runs` counts only the retained ones, and keyed
//! `host.quickstart.limits.plan.receipts_retention_days == 7` (pro: 30).

use crate::common;
use crate::sjrc;

use common::{McpClient, TestServer, extract_structured, signup, signup_and_make_pro};
use serde_json::json;
use sjrc::fire_until_receipts;

async fn retention_days(client: &McpClient) -> serde_json::Value {
    let quickstart = extract_structured(
        &client
            .tools_call("host.quickstart", json!({"kind": "echo"}))
            .await
            .expect("quickstart"),
    );
    quickstart["limits"]["plan"]["receipts_retention_days"].clone()
}

#[tokio::test]
async fn eight_day_old_receipts_are_swept_on_free_and_retention_is_a_plan_field() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "SJRC AC5 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "pinger", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    let set = extract_structured(
        &client
            .tools_call("host.trigger.set", json!({"tool": "pinger", "schedule": "0 * * * *"}))
            .await
            .expect("trigger.set"),
    );
    let trigger_id = set["id"].as_str().expect("id").to_string();
    fire_until_receipts(&client, &trigger_id, 1).await;

    // Two receipts that started 8 days ago, one a day ago (inside the 7 days).
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("find").expect("tenant");
    let now = mcphost::state::now_unix();
    for (n, age_days) in [(1, 8), (2, 8), (3, 1)] {
        server
            .state
            .db
            .insert_trigger_receipt(
                tenant.id,
                mcphost::receipts::ReceiptRow {
                    id: 0,
                    trigger_id: trigger_id.clone(),
                    tool_name: "pinger".into(),
                    run_id: format!("old-{n}"),
                    source: "schedule".into(),
                    started_unix: now - age_days * 86_400,
                    duration_ms: 1,
                    status: "done".into(),
                    error_code: None,
                    output_excerpt: String::new(),
                    cost_calls: 1,
                },
            )
            .await
            .expect("insert old receipt");
    }
    let before = extract_structured(
        &client
            .tools_call("host.schedule_receipts", json!({"trigger_id": trigger_id}))
            .await
            .expect("receipts"),
    );
    assert_eq!(before["summary"]["runs"], json!(4), "{before:?}");

    mcphost::channels::tick_once(&server.state).await.expect("housekeeping tick");

    let after = extract_structured(
        &client
            .tools_call("host.schedule_receipts", json!({"trigger_id": trigger_id}))
            .await
            .expect("receipts"),
    );
    assert_eq!(after["summary"]["runs"], json!(2), "only the retained rows count: {after:?}");
    let run_ids: Vec<&str> = after["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|r| r["run_id"].as_str().expect("run_id"))
        .collect();
    assert!(!run_ids.contains(&"old-1") && !run_ids.contains(&"old-2"), "{run_ids:?}");
    assert!(run_ids.contains(&"old-3"), "the 1-day-old receipt stays: {run_ids:?}");

    assert_eq!(retention_days(&client).await, json!(7));
    let (_ns, pro_key, _id) = signup_and_make_pro(&server, "SJRC AC5 Pro", "cus_sjrc5").await;
    let pro = McpClient::with_bearer(&server.base_url, &pro_key);
    assert_eq!(retention_days(&pro).await, json!(30));
}
