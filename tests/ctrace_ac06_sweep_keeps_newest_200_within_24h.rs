//! PRD-mcphost-call-trace-replay
//! AC6 -- Given 250 traced calls by one tenant and one trace older than 24 h,
//! When the housekeeping sweep runs, Then exactly the newest 200 within 24 h
//! remain and `calls` rows are untouched.

use crate::ctrace;
use crate::common;

use common::publish;
use serde_json::json;

const CALLS: usize = 250;
const KEEP: usize = 200;

#[tokio::test]
async fn sweep_leaves_the_newest_200_traces_within_24h_and_every_calls_row() {
    let _switch = ctrace::trace_switch().await;
    let fx = ctrace::start_pro().await;
    let tenant_id = fx.tenant_id().await;
    publish(&fx.client, "ping", "echo", json!({"schema": {"type": "object"}})).await;

    let mut ids: Vec<i64> = Vec::with_capacity(CALLS);
    for n in 0..CALLS {
        let result = fx.tool_call("ping", json!({"n": n})).await.expect("traced call");
        ids.push(result["call_id"].as_str().expect("call_id").parse().expect("numeric call_id"));
    }
    let db = &fx.server.state.db;
    assert_eq!(db.call_trace_ids_for_test(tenant_id).await.expect("trace ids"), ids, "every call wrote a trace");

    // One trace, from the newest 200, is older than 24 h.
    let stale = ids[CALLS - 10];
    db.age_call_trace_for_test(stale, mcphost::state::now_unix() - 25 * 3600).await.expect("age one trace");

    let calls_before = db.calls_row_count_for_test(tenant_id).await.expect("calls count");
    mcphost::channels::tick_once(&fx.server.state).await.expect("housekeeping tick");

    let mut expected: Vec<i64> = ids.iter().copied().filter(|id| *id != stale).collect();
    expected.drain(..expected.len() - KEEP);
    let kept = db.call_trace_ids_for_test(tenant_id).await.expect("trace ids after the sweep");
    assert_eq!(kept.len(), KEEP, "exactly {KEEP} traces remain");
    assert_eq!(kept, expected, "the newest {KEEP} within 24 h remain, the stale one does not");
    assert_eq!(
        db.calls_row_count_for_test(tenant_id).await.expect("calls count"),
        calls_before,
        "the sweep never touches calls rows"
    );
}
