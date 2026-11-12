//! PRD-mcphost-queue-primitive-v2
//! AC7 (P0) — Given tenant B's queue name, When tenant A calls any
//! `host.queue.*` with it, Then `queue_not_found`; Given a `consumer_tool` A
//! never published, When `create` runs, Then `tool_not_found`; and the three new
//! codes appear in `host.errors` (`error_registry`, `errors.json`).

use crate::qprim;
use qprim::{NO_TASKS, any_object, call, call_err, publish_consumer, rig, second_tenant};
use serde_json::{Value, json};

const NEW_CODES: [&str; 3] = ["queue_not_found", "queue_not_accepting", "queue_active"];

#[tokio::test]
async fn qprim_ac07_another_tenants_queue_name_is_queue_not_found_on_every_verb() {
    let rig = rig(NO_TASKS, |_| {}).await;
    let b = second_tenant(&rig).await;
    publish_consumer(&b, "b_tool", any_object(), 0).await;
    call(&b, "host.queue.create", json!({"name": "b_queue", "consumer_tool": "b_tool"})).await;
    call(&b, "host.queue.push", json!({"name": "b_queue", "items": [{"n": 1}]})).await;

    for (verb, args) in [
        ("stats", json!({"name": "b_queue"})),
        ("push", json!({"name": "b_queue", "items": [{"n": 2}]})),
        ("pause", json!({"name": "b_queue"})),
        ("resume", json!({"name": "b_queue"})),
        ("drain", json!({"name": "b_queue"})),
        ("delete", json!({"name": "b_queue"})),
    ] {
        let err = call_err(&rig.client, &format!("host.queue.{verb}"), args).await;
        assert_eq!(err.error_code.as_deref(), Some("queue_not_found"), "{verb}: {err:?}");
    }
    let listed = call(&rig.client, "host.queue.list", json!({})).await;
    assert_eq!(listed["queues"], json!([]), "A lists none of B's queues: {listed}");
    let theirs = call(&b, "host.queue.stats", json!({"name": "b_queue"})).await;
    assert_eq!((theirs["state"].clone(), theirs["pending"].clone()), (json!("active"), json!(1)), "B's queue is untouched: {theirs}");
}

#[tokio::test]
async fn qprim_ac07_a_consumer_the_tenant_never_published_is_tool_not_found() {
    let rig = rig(NO_TASKS, |_| {}).await;
    let b = second_tenant(&rig).await;
    publish_consumer(&b, "b_tool", any_object(), 0).await;
    for tool in ["never_published", "b_tool"] {
        let err = call_err(&rig.client, "host.queue.create", json!({"name": "q", "consumer_tool": tool})).await;
        assert_eq!(err.error_code.as_deref(), Some("tool_not_found"), "{tool}: {err:?}");
    }
}

#[tokio::test]
async fn qprim_ac07_the_three_new_codes_are_in_the_error_registry_and_in_errors_json() {
    let rig = rig(NO_TASKS, |_| {}).await;
    for code in NEW_CODES {
        assert!(mcphost::error_registry::lookup(code).is_some(), "{code} is a registry row");
    }
    let catalog: Value = reqwest::Client::new()
        .get(format!("{}/errors.json", rig.base_url))
        .send()
        .await
        .expect("GET /errors.json")
        .json()
        .await
        .expect("errors.json body");
    let listed = call(&rig.client, "host.errors", json!({})).await;
    for source in [&catalog, &listed] {
        let codes: Vec<&str> = source["codes"].as_array().expect("codes").iter().filter_map(|r| r["code"].as_str()).collect();
        for code in NEW_CODES {
            assert!(codes.contains(&code), "{code} is listed: {codes:?}");
        }
    }
}
