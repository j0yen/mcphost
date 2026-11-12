//! PRD-mcphost-queue-primitive-v2
//! AC6 (P1) — Given a `drained`, a `paused` and an `active` queue, When
//! `host.queue.delete` runs on each, Then the first two vanish with their items
//! and the third answers `queue_active` with the hint `pause or drain first`;
//! Given `done` items past the retention window, Then the nightly prune removes
//! them and keeps `pending` and `running` ones.

use crate::qprim;
use qprim::{NO_TASKS, any_object, call, call_err, free_plan, items, publish_consumer, rig, stats};
use serde_json::json;

const DAY_MS: i64 = 86_400_000;

#[tokio::test]
async fn qprim_ac06_drained_and_paused_queues_vanish_with_their_items_and_active_refuses() {
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
async fn qprim_ac06_the_nightly_prune_removes_old_finished_items_and_keeps_live_ones() {
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
