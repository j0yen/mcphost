//! PRD-mcphost-queue-primitive-v2
//! AC2 (P0) — Given a push of 1 000 items where items 3 and 500 violate the
//! consumer's schema, When it returns, Then `accepted` is 998, `rejected` names
//! indexes 3 and 500 with the schema message, and the 998 sit in `pending` in
//! push order (`idx` 0 to 997).

use crate::qprim;
use qprim::{NO_TASKS, call, publish_consumer, rig, stats};
use serde_json::{Value, json};

#[tokio::test]
async fn qprim_ac02_two_bad_items_of_a_thousand_are_named_and_the_rest_sit_in_order() {
    let rig = rig(NO_TASKS, |_| {}).await;
    let schema = json!({"type": "object", "properties": {"n": {"type": "integer"}}, "required": ["n"]});
    publish_consumer(&rig.client, "typed", schema, 0).await;
    call(&rig.client, "host.queue.create", json!({"name": "bulk", "consumer_tool": "typed"})).await;

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
