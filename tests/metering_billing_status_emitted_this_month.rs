//! Requirement 53 (P0, no dedicated AC number): "`billing.status` for a pro
//! tenant includes the tenant's ledgered emitted-call count for the current
//! month." A free tenant's `billing.status` carries no such field at all.

mod common;
use common::{McpClient, TestServer, record_ok_calls, signup, signup_and_make_pro};
use mcphost::billing::FakeBillingClient;
use mcphost::metering;
use serde_json::json;

#[tokio::test]
async fn pro_tenant_status_includes_emitted_this_month() {
    let server = TestServer::start().await;
    let (ns, key, tenant_id) =
        signup_and_make_pro(&server, "Status Emitted", "cus_status_emitted").await;
    record_ok_calls(&server, tenant_id, "some_tool", 12).await;
    let fake = FakeBillingClient::new(mcphost::state::now_unix());
    metering::run_once(&server.state.db, &fake, "mcphost_tool_calls")
        .await
        .expect("emit-meter run");

    let client = McpClient::with_bearer(&server.base_url, &key);
    let result = client
        .tools_call("billing.status", json!({}))
        .await
        .expect("billing.status");
    let status = common::extract_structured(&result);
    assert_eq!(status["plan"], json!("pro"));
    assert_eq!(status["metered_usage"]["emitted_this_month"], json!(12));
    let _ = ns;
}

#[tokio::test]
async fn free_tenant_status_has_no_metered_usage_field() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Status Free").await;

    let client = McpClient::with_bearer(&server.base_url, &key);
    let result = client
        .tools_call("billing.status", json!({}))
        .await
        .expect("billing.status");
    let status = common::extract_structured(&result);
    assert_eq!(status["plan"], json!("free"));
    assert!(
        status.get("metered_usage").is_none(),
        "a free tenant's status must not carry metered_usage at all: {status:?}"
    );
}
