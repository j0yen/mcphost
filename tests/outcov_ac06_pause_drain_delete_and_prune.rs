//! PRD-mcphost-outsch-ac01-coverage-diagnosable
//! AC6 (P0; v2 AC5 + AC6) — Given 4 running items, When `pause` is called, Then
//! none starts within 3 ticks and `resume` starts the next within 2 s; Given
//! `drain` on an empty active queue, Then it is `drained` and a `push` answers
//! `queue_not_accepting`; Given `delete` on a drained, a paused and an active
//! queue, Then the first two vanish and the third answers `queue_active`; Given
//! `done` items past retention, Then the prune removes them and keeps `pending`
//! and `running` ones.

use crate::qprim;
use qprim::{ALL_TASKS, NO_TASKS, any_object, call, call_err, free_plan, items, publish_consumer, rig, stats, wait_stats};
use serde_json::json;
use std::time::Duration;

const DAY_MS: i64 = 86_400_000;

#[tokio::test]
/// v2 AC5
async fn qprim_ac06a_pause_holds_the_next_item_and_resume_starts_it() {
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
/// v2 AC5
async fn qprim_ac06b_drain_on_an_empty_queue_is_drained_at_once_and_refuses_pushes() {
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
/// v2 AC5
async fn qprim_ac06c_drain_with_work_left_is_draining_then_drained_when_it_finishes() {
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

#[tokio::test]
/// v2 AC6
async fn qprim_ac06d_drained_and_paused_queues_vanish_with_their_items_and_active_refuses() {
    let rig = rig(NO_TASKS, |catalog| free_plan(catalog, |p| p.queues_max = 5)).await;
    publish_consumer(&rig.client, "one", any_object(), 0).await;
    for name in ["gone_drained", "gone_paused", "kept_active"] {
        call(&rig.client, "host.queue.create", json!({"name": name, "consumer_tool": "one"})).await;
    }
    call(&rig.client, "host.queue.drain", json!({"name": "gone_drained"})).await;
    call(&rig.client, "host.queue.push", json!({"name": "gone_paused", "items": items(3)})).await;
    call(&rig.client, "host.queue.pause", json!({"name": "gone_paused"})).await;
    call(&rig.client, "host.queue.push", json!({"name": "kept_active", "items": items(2)})).await;
    let paused_rows_before = rig.item_rows("gone_paused").await;
    assert_eq!(paused_rows_before.len(), 3);

    for name in ["gone_drained", "gone_paused"] {
        let deleted = call(&rig.client, "host.queue.delete", json!({"name": name})).await;
        assert_eq!(deleted["deleted"], json!(true), "{deleted}");
        let err = call_err(&rig.client, "host.queue.stats", json!({"name": name})).await;
        assert_eq!(err.error_code.as_deref(), Some("queue_not_found"), "{name}: {err:?}");
    }
    assert!(rig.item_rows("gone_paused").await.is_empty(), "the items went with the queue");
    assert!(rig.item_rows_by_queue_id(paused_rows_before[0].queue_id).await.is_empty());

    let err = call_err(&rig.client, "host.queue.delete", json!({"name": "kept_active"})).await;
    assert_eq!(err.error_code.as_deref(), Some("queue_active"), "{err:?}");
    assert_eq!(err.data["hint"], json!("pause or drain first"), "{err:?}");
    assert_eq!(stats(&rig.client, "kept_active").await["pending"], json!(2), "the active queue is untouched");

    let listed = call(&rig.client, "host.queue.list", json!({})).await;
    let names: Vec<&str> = listed["queues"].as_array().expect("queues").iter().filter_map(|q| q["name"].as_str()).collect();
    assert_eq!(names, vec!["kept_active"], "{listed}");
}

#[tokio::test]
/// v2 AC6
async fn qprim_ac06e_the_nightly_prune_removes_old_finished_items_and_keeps_live_ones() {
    let rig = rig(NO_TASKS, |_| {}).await;
    publish_consumer(&rig.client, "one", any_object(), 0).await;
    call(&rig.client, "host.queue.create", json!({"name": "aging", "consumer_tool": "one"})).await;
    let now_ms = mcphost::state::now_unix_ms();
    let window_days = mcphost::retention::POLICY_TABLES
        .iter()
        .find(|t| t.name == "queue_items")
        .expect("queue_items is a retention policy table")
        .default_days;
    assert_eq!(window_days, 30, "the same window as runs");
    let old = now_ms - (window_days + 1) * DAY_MS;
    let recent = now_ms - DAY_MS;
    for (status, ended) in [
        ("done", Some(old)),
        ("failed", Some(old)),
        ("done", Some(recent)),
        ("pending", None),
        ("running", None),
    ] {
        rig.seed_item("aging", status, ended).await;
    }
    assert_eq!(rig.item_rows("aging").await.len(), 5);

    // `serve` seeds the policy rows from `POLICY_TABLES` at startup; a bare test state does it here.
    rig.state.db.seed_retention_policy_from_env().await.expect("seed retention policy");
    let report = rig.state.db.prune_once().await.expect("prune");
    assert_eq!(report.deleted.get("queue_items"), Some(&2), "{:?}", report.deleted);

    let left: Vec<String> = rig.item_rows("aging").await.into_iter().map(|r| r.status).collect();
    assert_eq!(left.len(), 3, "{left:?}");
    for status in ["done", "pending", "running"] {
        assert!(left.iter().any(|s| s == status), "{status} survives: {left:?}");
    }
    assert!(!left.iter().any(|s| s == "failed"), "the old failed item went: {left:?}");
}
