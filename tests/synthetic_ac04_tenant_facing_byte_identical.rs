//! PRD-mcphost-synthetic-flag
//! AC4 — Given a labeled and an unlabeled tenant with identical state,
//! When each calls `billing.status`, `tools/list`, and publishes a tool,
//! Then the responses differ only in tenant-identifying fields -- the
//! label appears in none of them.

mod common;
use common::{McpClient, TestServer, extract_structured, signup_with_synthetic_header};
use serde_json::{Value, json};

/// Every top-level key of `v`, sorted -- used to assert two responses have
/// the same *shape* regardless of tenant-identifying values.
fn keys(v: &Value) -> Vec<String> {
    let mut ks: Vec<String> = v.as_object().expect("object").keys().cloned().collect();
    ks.sort();
    ks
}

/// The label must never appear as a string anywhere in the response, under
/// any key name -- not just absent from a `synthetic` key.
fn contains_label_anywhere(v: &Value, label: &str) -> bool {
    match v {
        Value::String(s) => s.contains(label),
        Value::Array(a) => a.iter().any(|x| contains_label_anywhere(x, label)),
        Value::Object(o) => o.values().any(|x| contains_label_anywhere(x, label)),
        _ => false,
    }
}

#[tokio::test]
async fn labeled_and_unlabeled_tenant_facing_responses_match_shape() {
    let server = TestServer::start().await;
    let label = "synthorg:ac4-should-never-leak";

    let labeled_signup =
        signup_with_synthetic_header(&server.base_url, "Labeled Caller", label).await;
    let labeled_key = labeled_signup["key"].as_str().expect("key").to_string();
    let labeled = McpClient::with_bearer(&server.base_url, &labeled_key);

    let unlabeled_signup =
        signup_with_synthetic_header(&server.base_url, "Unlabeled Caller", "").await;
    let unlabeled_key = unlabeled_signup["key"].as_str().expect("key").to_string();
    let unlabeled = McpClient::with_bearer(&server.base_url, &unlabeled_key);

    // billing.status
    let labeled_status = extract_structured(
        &labeled
            .tools_call("billing.status", json!({}))
            .await
            .expect("labeled billing.status"),
    );
    let unlabeled_status = extract_structured(
        &unlabeled
            .tools_call("billing.status", json!({}))
            .await
            .expect("unlabeled billing.status"),
    );
    assert_eq!(keys(&labeled_status), keys(&unlabeled_status));
    assert!(!contains_label_anywhere(&labeled_status, label));

    // tools/list (empty, before any publish -- same shape either way)
    let labeled_list = labeled.tools_list().await.expect("labeled tools/list");
    let unlabeled_list = unlabeled.tools_list().await.expect("unlabeled tools/list");
    assert_eq!(keys(&labeled_list), keys(&unlabeled_list));
    assert!(!contains_label_anywhere(&labeled_list, label));

    // host.tool_publish
    let publish_spec = json!({"schema": {"type": "object"}});
    let labeled_publish = extract_structured(
        &labeled
            .tools_call(
                "host.tool_publish",
                json!({"name": "hello", "kind": "echo", "spec": publish_spec}),
            )
            .await
            .expect("labeled publish"),
    );
    let unlabeled_publish = extract_structured(
        &unlabeled
            .tools_call(
                "host.tool_publish",
                json!({"name": "hello", "kind": "echo", "spec": publish_spec}),
            )
            .await
            .expect("unlabeled publish"),
    );
    assert_eq!(keys(&labeled_publish), keys(&unlabeled_publish));
    assert!(!contains_label_anywhere(&labeled_publish, label));
}
