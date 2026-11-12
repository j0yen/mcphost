//! PRD-mcphost-outsch-ac01-coverage-diagnosable
//! AC4 (P0; v2 AC2 + AC3) — Given a paused queue and a push of 1 000 items where
//! items 3 and 500 break the consumer's schema, When it returns, Then `accepted`
//! is 998, `rejected` names indexes 3 and 500, and the 998 sit `pending` in push
//! order; Given a free tenant with 2 queues, When a third `create` runs, Then
//! `quota_exceeded` with `data.limit {name: "queues_max", value: 2}` matching
//! `limits.plan.queues_max`, and a push past `queue_items_max` inserts nothing
//! and names it.

use crate::qprim;
use qprim::{NO_TASKS, any_object, call, call_err, items, publish_consumer, rig, stats};
use serde_json::{Value, json};

#[tokio::test]
/// v2 AC2
async fn qprim_ac04a_two_bad_items_of_a_thousand_are_named_and_the_rest_sit_in_order() {
    let rig = rig(NO_TASKS, |_| {}).await;
    let schema = json!({"type": "object", "properties": {"n": {"type": "integer"}}, "required": ["n"]});
    publish_consumer(&rig.client, "typed", schema, 0).await;
    call(&rig.client, "host.queue.create", json!({"name": "bulk", "consumer_tool": "typed"})).await;
    call(&rig.client, "host.queue.pause", json!({"name": "bulk"})).await;

    let items: Vec<Value> = (0..1000).map(|i| if i == 3 || i == 500 { json!({"n": 4.78}) } else { json!({"n": i}) }).collect();
    let pushed = call(&rig.client, "host.queue.push", json!({"name": "bulk", "items": items})).await;

    assert_eq!(pushed["accepted"], json!(998), "{pushed}");
    let rejected = pushed["rejected"].as_array().expect("rejected array");
    let indexes: Vec<u64> = rejected.iter().map(|r| r["index"].as_u64().expect("index")).collect();
    assert_eq!(indexes, vec![3, 500], "{pushed}");
    for r in rejected {
        let reason = r["reason"].as_str().expect("reason");
        assert!(reason.starts_with("args_invalid: ") && reason.contains("4.78 is not of type \"integer\""), "{reason}");
    }
    assert_eq!((pushed["first_idx"].clone(), pushed["last_idx"].clone()), (json!(0), json!(997)), "{pushed}");
    assert_eq!(stats(&rig.client, "bulk").await["pending"], json!(998));

    let rows = rig.item_rows("bulk").await;
    assert_eq!(rows.len(), 998);
    let expected = (0..1000).filter(|i| *i != 3 && *i != 500);
    for (position, (row, n)) in rows.iter().zip(expected).enumerate() {
        assert_eq!(row.idx, position as i64, "idx runs 0 to 997 in push order");
        assert_eq!(row.status, "pending");
        assert_eq!(serde_json::from_str::<Value>(&row.item_json).expect("item json"), json!({"n": n}));
    }
}

#[tokio::test]
/// v2 AC3
async fn qprim_ac04b_a_third_queue_on_free_is_quota_exceeded_naming_queues_max() {
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
/// v2 AC3
async fn qprim_ac04c_a_push_past_queue_items_max_inserts_nothing_and_names_it() {
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
/// v2 AC3 (R3: `queue_item_bytes_max`)
async fn qprim_ac04d_an_oversize_item_is_rejected_by_its_index() {
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
