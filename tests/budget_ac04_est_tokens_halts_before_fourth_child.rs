//! PRD-mcphost-run-budget-governor
//! AC4 (P0) — Given a chain whose third child returns a 400 KB result, When
//! `max_est_tokens` is 150,000, Then the run halts before the fourth child
//! with dimension `est_tokens` and `used` ≥ 100,000.
//!
//! `echo` returns its own args verbatim (`kinds::echo`), so a step whose
//! args carry a 400 KB string returns that same 400 KB string as its
//! result: `ceil((args_bytes + result_bytes) / 4)` alone, from that one
//! child, already exceeds `max_est_tokens: 150_000` -- steps one and two
//! stay trivial (a handful of bytes), so the ledger's `est_tokens` crosses
//! the limit exactly at step three, before step four is ever dispatched.

use crate::common;
use common::{TestServer, chain_kind_registry, extract_structured, publish, signup};
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn est_tokens_halts_before_the_fourth_child() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "AC4 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let schema = json!({"type": "object"});
    for i in 0..4 {
        publish(&client, &format!("step{i}"), "echo", json!({"schema": schema})).await;
    }
    // 400 KB of payload, mapped from the chain's own call args ($.input) so
    // the STORED spec stays small (a literal here would trip
    // MAX_SPEC_BYTES at publish time) -- only step2 (0-based -- the third
    // step) touches it, so steps 0/1/3 stay trivial and never dispatch a
    // large result of their own.
    let steps = json!([
        {"tool": "step0", "args": {"n": 1}},
        {"tool": "step1", "args": {"n": 2}},
        {"tool": "step2", "args": {"blob": "$.input.blob"}},
        {"tool": "step3", "args": {"n": 3}},
    ]);
    let chain = publish(&client, "pipeline", "chain", json!({"steps": steps})).await;
    assert_eq!(chain, format!("{ns}.pipeline"));

    let blob = "x".repeat(400 * 1024);
    let enqueue = extract_structured(
        &client
            .tools_call(
                "host.tool_call",
                json!({
                    "name": "pipeline",
                    "args": {"blob": blob},
                    "async": true,
                    "budget": {"max_est_tokens": 150_000},
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
        json!("est_tokens"),
        "error_data_json.dimension must name est_tokens: {done}"
    );
    let used = done["error"]["data"]["used"].as_i64().expect("used is an integer");
    assert!(used >= 100_000, "used must be at least 100,000: {done}");

    // Only three children ran -- the fourth's own step count never
    // contributed a fourth `child_calls` record.
    let budget = &done["progress"]["budget"];
    assert_eq!(budget["used"]["child_calls"], json!(3), "{budget}");
}
