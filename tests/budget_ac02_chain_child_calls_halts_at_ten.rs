//! PRD-mcphost-run-budget-governor
//! AC2 (P0) — Given a chain of 12 steps and `budget: {max_child_calls: 10}`,
//! When it runs, Then exactly 10 children execute, the run ends `error`
//! with `error_class` `budget_exceeded` and `error_data_json.dimension`
//! `child_calls`.
//!
//! Run as an async `host.tool_call` (the PRD's own `budget` argument is
//! read at enqueue time, `runs::execute_job` is the dispatch path a `runs`
//! row's own error_class/error come from) -- a synchronous chain call also
//! writes a `runs` row (`runs_ac05`), but only the async path's executor
//! finalizes it with a named `error_class`/`error` the way this AC checks.

use crate::common;
use common::{TestServer, chain_kind_registry, extract_structured, publish, signup};
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn chain_of_twelve_halts_at_ten_children_with_budget_exceeded() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "AC2 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    // Twelve trivial echo steps, each one child call in the chain's tree.
    let schema = json!({"type": "object"});
    for i in 0..12 {
        publish(&client, &format!("step{i}"), "echo", json!({"schema": schema})).await;
    }
    let steps: Vec<serde_json::Value> = (0..12)
        .map(|i| json!({"tool": format!("step{i}"), "args": {"n": i}}))
        .collect();
    let chain = publish(&client, "pipeline", "chain", json!({"steps": steps})).await;
    assert_eq!(chain, format!("{ns}.pipeline"));

    let enqueue = extract_structured(
        &client
            .tools_call(
                "host.tool_call",
                json!({
                    "name": "pipeline",
                    "args": {},
                    "async": true,
                    "budget": {"max_child_calls": 10},
                }),
            )
            .await
            .expect("enqueue ok"),
    );
    let run_id = enqueue["run_id"].as_str().expect("run_id").to_string();

    let deadline = Instant::now() + Duration::from_secs(15);
    let done = loop {
        let got = extract_structured(
            &client
                .tools_call("host.runs.get", json!({"run_id": run_id}))
                .await
                .expect("runs.get ok"),
        );
        if got["status"] != json!("queued") && got["status"] != json!("running") {
            break got;
        }
        assert!(Instant::now() < deadline, "run never finalized within 15s");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    assert_eq!(done["status"], json!("error"), "{done}");
    assert_eq!(done["error_class"], json!("budget_exceeded"), "{done}");
    assert_eq!(
        done["error"]["data"]["dimension"],
        json!("child_calls"),
        "error_data_json.dimension must name child_calls: {done}"
    );
    assert_eq!(done["error"]["data"]["limit"], json!(10), "{done}");
    assert_eq!(done["error"]["data"]["used"], json!(10), "{done}");

    let budget = &done["progress"]["budget"];
    assert_eq!(budget["verdict"], json!("exceeded"), "{budget}");
    assert_eq!(budget["used"]["child_calls"], json!(10), "{budget}");
}
