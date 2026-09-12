//! PRD-mcphost-synthetic-flag
//! AC2 — Given a signup request with header
//! `x-mcphost-synthetic: synthorg:run-a`, When signup completes, Then the
//! tenant row stores `synthorg:run-a` and the signup response is
//! byte-identical to an unlabeled signup's shape.

use crate::common;
use common::{TestServer, signup, signup_with_synthetic_header};
use serde_json::Value;

/// The set of top-level keys a `signup` response carries -- used to assert
/// "byte-identical shape" without depending on values that legitimately
/// differ between two signups (namespace, key).
fn keys(v: &Value) -> Vec<String> {
    let mut ks: Vec<String> = v
        .as_object()
        .expect("object")
        .keys()
        .cloned()
        .collect();
    ks.sort();
    ks
}

#[tokio::test]
async fn labeled_signup_stores_label_and_matches_response_shape() {
    let server = TestServer::start().await;

    let (unlabeled_ns, _) = signup(&server.base_url, "Unlabeled Agent").await;
    let labeled = signup_with_synthetic_header(
        &server.base_url,
        "Labeled Agent",
        "synthorg:run-a",
    )
    .await;

    let unlabeled_result = common::McpClient::new(&server.base_url)
        .tools_call("signup", serde_json::json!({"name": "Shape Reference"}))
        .await
        .expect("reference signup");
    let unlabeled_shape = common::extract_structured(&unlabeled_result);

    assert_eq!(
        keys(&labeled),
        keys(&unlabeled_shape),
        "labeled and unlabeled signup responses must have identical top-level shape"
    );
    assert!(
        labeled.get("synthetic").is_none(),
        "the label must never appear in the signup response itself"
    );

    let labeled_ns = labeled["tenant"].as_str().expect("tenant field").to_string();
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(labeled_ns)
        .await
        .expect("query")
        .expect("labeled tenant exists");
    assert_eq!(tenant.synthetic.as_deref(), Some("synthorg:run-a"));

    let unlabeled_tenant = server
        .state
        .db
        .find_tenant_by_namespace(unlabeled_ns)
        .await
        .expect("query")
        .expect("unlabeled tenant exists");
    // PRD-mcphost-tenant-attribution requirement 1 / AC1: a signup with no
    // explicit stamp from this suite's loopback test server derives
    // `source_class = loopback` and defaults `synthetic` to
    // `harness:unstamped`, not `None` -- `None` is reserved for a
    // genuinely `external` tenant now.
    assert_eq!(
        unlabeled_tenant.synthetic.as_deref(),
        Some("harness:unstamped")
    );
}
