//! PRD-mcphost-synthetic-flag
//! AC8 — Given the admin tenant listing used for exports, When read with
//! labeled and unlabeled tenants present, Then every row carries the
//! `synthetic` field, null for unlabeled.

mod common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn every_row_carries_synthetic_null_for_unlabeled() {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    let (ns_labeled, _) = signup(&server.base_url, "Labeled Export").await;
    let (ns_unlabeled, _) = signup(&server.base_url, "Unlabeled Export").await;
    admin
        .tools_call(
            "admin.tenant_set_synthetic",
            json!({"tenant": ns_labeled, "label": "operator"}),
        )
        .await
        .expect("label tenant");

    let result = admin
        .tools_call("admin.tenants", json!({}))
        .await
        .expect("admin.tenants");
    let list = extract_structured(&result);
    let tenants = list["tenants"].as_array().expect("tenants array");
    assert!(tenants.len() >= 2);
    for row in tenants {
        assert!(
            row.get("synthetic").is_some(),
            "every row must carry a synthetic field (present, possibly null): {row:?}"
        );
    }
    let labeled_row = tenants
        .iter()
        .find(|t| t["tenant"] == json!(ns_labeled))
        .expect("labeled row");
    assert_eq!(labeled_row["synthetic"], json!("operator"));
    let unlabeled_row = tenants
        .iter()
        .find(|t| t["tenant"] == json!(ns_unlabeled))
        .expect("unlabeled row");
    assert_eq!(unlabeled_row["synthetic"], json!(null));
}
