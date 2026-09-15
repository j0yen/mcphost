//! AC1 — Given a live free-plan tenant created via public `signup`, When it
//! calls `host.self_offboard(tenant_key)` with its own key, Then the
//! tenant's row shows `disabled=1` and the call returns success.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn self_offboard_disables_the_calling_tenant() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Leaving Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let result = client
        .tools_call("host.self_offboard", json!({}))
        .await
        .expect("host.self_offboard must succeed for a live tenant's own key");
    let structured = common::extract_structured(&result);

    assert_eq!(structured["tenant"], json!(ns));
    assert_eq!(structured["disabled"], json!(true));
    assert_eq!(structured["disabled_reason"], json!("self_offboard"));

    // Belt and suspenders: the row itself, not just the response body,
    // shows disabled=1 with the self_offboard reason.
    let row = server
        .state
        .db
        .find_tenant_by_namespace(ns.clone())
        .await
        .expect("find tenant")
        .expect("tenant row still exists");
    assert!(row.disabled, "tenant row must be disabled after self_offboard");
    assert_eq!(row.disabled_reason.as_deref(), Some("self_offboard"));
}
