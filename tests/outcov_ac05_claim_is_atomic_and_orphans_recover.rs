//! PRD-mcphost-outsch-ac01-coverage-diagnosable
//! AC5 (P0; v2 AC4) — Given two dispatcher ticks invoked concurrently over the same
//! pending item, When both run, Then exactly one `runs` row exists for it and the
//! item is `running` once; Given a `running` item whose run row is missing, Then
//! the next tick returns it to `pending`.

use crate::common;
use crate::qprim;
use qprim::{NO_TASKS, any_object, call, publish_consumer, rig, stats};
use serde_json::json;

async fn queue_runs(client: &common::McpClient) -> Vec<serde_json::Value> {
    call(client, "host.runs.list", json!({"trigger": "queue"})).await["runs"].as_array().expect("runs").clone()
}

#[tokio::test]
/// v2 AC4
async fn qprim_ac05a_two_concurrent_ticks_start_one_item_once() {
    let rig = rig(NO_TASKS, |_| {}).await;
    publish_consumer(&rig.client, "one", any_object(), 0).await;
    call(&rig.client, "host.queue.create", json!({"name": "race", "consumer_tool": "one", "concurrency": 2})).await;
    call(&rig.client, "host.queue.push", json!({"name": "race", "items": [{"n": 1}]})).await;

    let (a, b) = tokio::join!(mcphost::queues::tick(&rig.state), mcphost::queues::tick(&rig.state));
    a.expect("first tick");
    b.expect("second tick");

    let runs = queue_runs(&rig.client).await;
    assert_eq!(runs.len(), 1, "exactly one runs row for the item: {runs:?}");
    let s = stats(&rig.client, "race").await;
    assert_eq!((s["running"].clone(), s["pending"].clone(), s["attempts_total"].clone()), (json!(1), json!(0), json!(1)), "{s}");
    let rows = rig.item_rows("race").await;
    assert_eq!(rows[0].run_id.as_deref(), runs[0]["run_id"].as_str(), "the item names the one run");
}

#[tokio::test]
/// v2 AC4
async fn qprim_ac05b_a_running_item_with_no_run_row_returns_to_pending() {
    let rig = rig(NO_TASKS, |_| {}).await;
    publish_consumer(&rig.client, "one", any_object(), 0).await;
    call(&rig.client, "host.queue.create", json!({"name": "orphan", "consumer_tool": "one"})).await;
    call(&rig.client, "host.queue.pause", json!({"name": "orphan"})).await;
    rig.seed_orphan("orphan", "run_that_was_never_written").await;
    assert_eq!(stats(&rig.client, "orphan").await["running"], json!(1));

    mcphost::queues::tick(&rig.state).await.expect("tick");
    let s = stats(&rig.client, "orphan").await;
    assert_eq!((s["pending"].clone(), s["running"].clone()), (json!(1), json!(0)), "the next tick returns it to pending: {s}");

    // Resumed, the same item starts again, under a run row that exists.
    call(&rig.client, "host.queue.resume", json!({"name": "orphan"})).await;
    mcphost::queues::tick(&rig.state).await.expect("tick");
    let runs = queue_runs(&rig.client).await;
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert_ne!(runs[0]["run_id"], json!("run_that_was_never_written"));
    assert_eq!(stats(&rig.client, "orphan").await["running"], json!(1));
}
