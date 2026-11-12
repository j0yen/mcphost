//! PRD-mcphost-queue-primitive-v2
//! AC5 (P0) — Given an active queue with 4 items running, When `pause` is
//! called, Then no item starts within 3 ticks, the 4 finish as `done`, and
//! `resume` starts the next within 2 s; Given `drain` on an empty active queue,
//! Then `state` is `drained` with `drained_at_ms` set and a `push` answers
//! `queue_not_accepting`.

use crate::qprim;
use qprim::{ALL_TASKS, any_object, call, call_err, free_plan, items, publish_consumer, rig, stats, wait_stats};
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn qprim_ac05_pause_holds_the_next_item_and_resume_starts_it() {
    let rig = rig(ALL_TASKS, |catalog| {
        free_plan(catalog, |p| {
            p.jobs_concurrent = 4;
            p.queue_concurrency_max = 4;
        });
    })
    .await;
    publish_consumer(&rig.client, "sleeper", any_object(), 1_500).await;
    call(&rig.client, "host.queue.create", json!({"name": "batch", "consumer_tool": "sleeper", "concurrency": 4})).await;
    call(&rig.client, "host.queue.push", json!({"name": "batch", "items": items(8)})).await;

    wait_stats(&rig.client, "batch", 10, |s| s["running"] == json!(4)).await;
    let paused = call(&rig.client, "host.queue.pause", json!({"name": "batch"})).await;
    assert_eq!(paused["state"], json!("paused"), "{paused}");

    // Three dispatcher ticks (1 s each) pass; the four running items finish; nothing new starts.
    let window = tokio::time::Instant::now() + Duration::from_millis(3_400);
    while tokio::time::Instant::now() < window {
        let s = stats(&rig.client, "batch").await;
        assert_eq!(s["attempts_total"], json!(4), "no item may start while paused: {s}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let held = stats(&rig.client, "batch").await;
    assert_eq!((held["done"].clone(), held["running"].clone(), held["pending"].clone()), (json!(4), json!(0), json!(4)), "{held}");

    call(&rig.client, "host.queue.resume", json!({"name": "batch"})).await;
    let resumed = wait_stats(&rig.client, "batch", 2, |s| s["attempts_total"].as_i64().is_some_and(|n| n > 4)).await;
    assert_eq!(resumed["state"], json!("active"), "{resumed}");
}

#[tokio::test]
async fn qprim_ac05_drain_on_an_empty_queue_is_drained_at_once_and_refuses_pushes() {
    let rig = rig(ALL_TASKS, |_| {}).await;
    publish_consumer(&rig.client, "one", any_object(), 0).await;
    call(&rig.client, "host.queue.create", json!({"name": "done_soon", "consumer_tool": "one"})).await;

    let drained = call(&rig.client, "host.queue.drain", json!({"name": "done_soon"})).await;
    assert_eq!(drained["state"], json!("drained"), "{drained}");
    assert!(drained["drained_at_ms"].as_i64().is_some_and(|t| t > 0), "{drained}");

    let err = call_err(&rig.client, "host.queue.push", json!({"name": "done_soon", "items": items(1)})).await;
    assert_eq!(err.error_code.as_deref(), Some("queue_not_accepting"), "{err:?}");
}

#[tokio::test]
async fn qprim_ac05_drain_with_work_left_is_draining_then_drained_when_it_finishes() {
    let rig = rig(ALL_TASKS, |_| {}).await;
    publish_consumer(&rig.client, "quick", any_object(), 200).await;
    call(&rig.client, "host.queue.create", json!({"name": "wind_down", "consumer_tool": "quick"})).await;
    call(&rig.client, "host.queue.push", json!({"name": "wind_down", "items": items(2)})).await;

    let draining = call(&rig.client, "host.queue.drain", json!({"name": "wind_down"})).await;
    assert_eq!(draining["state"], json!("draining"), "{draining}");
    let end = wait_stats(&rig.client, "wind_down", 15, |s| s["state"] == json!("drained")).await;
    assert_eq!((end["done"].clone(), end["pending"].clone()), (json!(2), json!(0)), "{end}");
    assert!(end["drained_at_ms"].as_i64().is_some(), "{end}");
}
