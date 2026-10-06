//! PRD-mcphost-activation-funnel
//! AC6 (P0) — Given a stage with two tenants, When `admin.funnel` is
//! read, Then that stage's `median_minutes` is null and its count is 2.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn a_stage_with_two_tenants_reports_null_median() {
    let server = TestServer::start().await;

    // Exactly two tenants ever make an authenticated call on this fresh
    // server, so `first_call`'s count is exactly 2 -- one short of
    // requirement 3's `median_minutes: null` floor of 3.
    let (_ns1, key1) = signup(&server.base_url, "AC6 Tenant One").await;
    let (_ns2, key2) = signup(&server.base_url, "AC6 Tenant Two").await;
    McpClient::with_bearer(&server.base_url, &key1)
        .tools_call("host.whoami", json!({}))
        .await
        .expect("tenant one's first authenticated call");
    McpClient::with_bearer(&server.base_url, &key2)
        .tools_call("host.whoami", json!({}))
        .await
        .expect("tenant two's first authenticated call");

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let funnel = extract_structured(
        &admin.tools_call("admin.funnel", json!({"days": 7})).await.expect("admin.funnel"),
    );
    assert_eq!(funnel["signups"], json!(2), "{funnel}");
    let stages = funnel["stages"].as_array().expect("stages array");
    let first_call = stages
        .iter()
        .find(|s| s["name"] == json!("first_call"))
        .expect("first_call stage present");
    assert_eq!(first_call["count"], json!(2), "{funnel}");
    assert!(
        first_call["median_minutes"].is_null(),
        "a 2-tenant stage must report median_minutes: null, not a real median: {funnel}"
    );
}
