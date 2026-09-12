//! PRD-mcphost-synthetic-flag
//! AC5 — Given `admin.tenant_set_synthetic` with a label and then with
//! null, When applied to one tenant, Then the row updates each time and
//! `admin.tenants` reflects it.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn set_then_clear_updates_row_and_admin_tenants_listing() {
    let server = TestServer::start().await;
    let (ns, _) = signup(&server.base_url, "Retag Me").await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    let set_result = admin
        .tools_call(
            "admin.tenant_set_synthetic",
            json!({"tenant": ns, "label": "synthorg:manual"}),
        )
        .await
        .expect("set label");
    let set_structured = extract_structured(&set_result);
    assert_eq!(set_structured["synthetic"], json!("synthorg:manual"));

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns.clone())
        .await
        .expect("query")
        .expect("tenant exists");
    assert_eq!(tenant.synthetic.as_deref(), Some("synthorg:manual"));

    let list_result = admin
        .tools_call("admin.tenants", json!({}))
        .await
        .expect("admin.tenants");
    let list = extract_structured(&list_result);
    let row = list["tenants"]
        .as_array()
        .expect("tenants array")
        .iter()
        .find(|t| t["tenant"] == json!(ns))
        .expect("row for ns");
    assert_eq!(row["synthetic"], json!("synthorg:manual"));

    // Clearing with `label: null` must flip the row back to unlabeled.
    let clear_result = admin
        .tools_call(
            "admin.tenant_set_synthetic",
            json!({"tenant": ns, "label": null}),
        )
        .await
        .expect("clear label");
    let clear_structured = extract_structured(&clear_result);
    assert_eq!(clear_structured["synthetic"], json!(null));

    let tenant_after = server
        .state
        .db
        .find_tenant_by_namespace(ns.clone())
        .await
        .expect("query")
        .expect("tenant exists");
    assert_eq!(tenant_after.synthetic, None);

    let list_result2 = admin
        .tools_call("admin.tenants", json!({}))
        .await
        .expect("admin.tenants");
    let list2 = extract_structured(&list_result2);
    let row2 = list2["tenants"]
        .as_array()
        .expect("tenants array")
        .iter()
        .find(|t| t["tenant"] == json!(ns))
        .expect("row for ns");
    assert_eq!(row2["synthetic"], json!(null));
}

#[tokio::test]
async fn invalid_label_is_rejected_and_unknown_tenant_errors() {
    let server = TestServer::start().await;
    let (ns, _) = signup(&server.base_url, "Bad Label Target").await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    let err = admin
        .tools_call(
            "admin.tenant_set_synthetic",
            json!({"tenant": ns, "label": "Bad Label!"}),
        )
        .await
        .expect_err("invalid label must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("invalid_params"));

    let err2 = admin
        .tools_call(
            "admin.tenant_set_synthetic",
            json!({"tenant": "t_doesnotexist", "label": "operator"}),
        )
        .await
        .expect_err("unknown tenant must error");
    assert_eq!(err2.error_code.as_deref(), Some("tool_not_found"));
}
