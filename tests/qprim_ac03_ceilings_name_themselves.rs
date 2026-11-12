//! PRD-mcphost-queue-primitive-v2
//! AC3 (P0) — Given a free tenant with 2 queues, When a third
//! `host.queue.create` runs, Then `quota_exceeded` with `data.limit {name:
//! "queues_max", value: 2}`, `limits.plan.queues_max` in `host.quickstart` is 2,
//! and a push past `queue_items_max` inserts nothing and names `queue_items_max`.

use crate::qprim;
use qprim::{NO_TASKS, any_object, call, call_err, items, publish_consumer, rig, stats};
use serde_json::json;

#[tokio::test]
async fn qprim_ac03_a_third_queue_on_free_is_quota_exceeded_naming_queues_max() {
    let rig = rig(NO_TASKS, |_| {}).await;
    publish_consumer(&rig.client, "one", any_object(), 0).await;
    for name in ["first", "second"] {
        call(&rig.client, "host.queue.create", json!({"name": name, "consumer_tool": "one"})).await;
    }
    let err = call_err(&rig.client, "host.queue.create", json!({"name": "third", "consumer_tool": "one"})).await;
    assert_eq!(err.error_code.as_deref(), Some("quota_exceeded"), "{err:?}");
    assert_eq!(err.data["limit"], json!({"name": "queues_max", "value": 2}), "{err:?}");
    assert_eq!(err.data["used"], json!(2), "{err:?}");

    let quickstart = call(&rig.client, "host.quickstart", json!({"kind": "echo"})).await;
    let plan = &quickstart["limits"]["plan"];
    assert_eq!(plan["queues_max"], json!(2), "{plan}");
    for key in ["queue_items_max", "queue_concurrency_max", "queue_item_bytes_max"] {
        assert!(plan[key].is_i64(), "limits.plan lists {key}: {plan}");
    }
}

#[tokio::test]
async fn qprim_ac03_a_push_past_queue_items_max_inserts_nothing_and_names_it() {
    let rig = rig(NO_TASKS, |_| {}).await;
    publish_consumer(&rig.client, "one", any_object(), 0).await;
    call(&rig.client, "host.queue.create", json!({"name": "full", "consumer_tool": "one"})).await;
    let cap = rig.state.plans.get("free").expect("free plan").queue_items_max;
    assert_eq!(cap, 1000);

    let filled = call(&rig.client, "host.queue.push", json!({"name": "full", "items": items(1000)})).await;
    assert_eq!(filled["accepted"], json!(1000), "{filled}");

    let err = call_err(&rig.client, "host.queue.push", json!({"name": "full", "items": items(3)})).await;
    assert_eq!(err.error_code.as_deref(), Some("quota_exceeded"), "{err:?}");
    assert_eq!(err.data["limit"], json!({"name": "queue_items_max", "value": 1000}), "{err:?}");
    assert_eq!(stats(&rig.client, "full").await["pending"], json!(1000), "the refused push inserted nothing");
}

#[tokio::test]
async fn qprim_ac03_an_oversize_item_is_rejected_by_its_index() {
    let rig = rig(NO_TASKS, |_| {}).await;
    publish_consumer(&rig.client, "one", any_object(), 0).await;
    call(&rig.client, "host.queue.create", json!({"name": "wide", "consumer_tool": "one"})).await;
    let big = json!({"blob": "x".repeat(5_000)});
    let pushed = call(&rig.client, "host.queue.push", json!({"name": "wide", "items": [{"n": 1}, big]})).await;
    assert_eq!(pushed["accepted"], json!(1), "{pushed}");
    let reason = pushed["rejected"][0]["reason"].as_str().expect("reason");
    assert_eq!(pushed["rejected"][0]["index"], json!(1));
    assert!(reason.contains("queue_item_bytes_max"), "{reason}");
}
