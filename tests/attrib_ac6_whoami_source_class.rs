//! PRD-mcphost-tenant-attribution
//! AC6 (P1) — Given an authenticated tenant, When it calls `host.whoami`,
//! Then the response includes its `source_class` and recorded client
//! name.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn whoami_reports_source_class_and_client() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url).with_client_info("claude-code", "2.1");

    let signup_result = client
        .tools_call("signup", json!({"name": "Whoami Caller"}))
        .await
        .expect("signup");
    let key = extract_structured(&signup_result)["key"]
        .as_str()
        .expect("key field")
        .to_string();

    let authed = McpClient::with_bearer(&server.base_url, &key).with_client_info("claude-code", "2.1");
    let whoami_result = authed
        .tools_call("host.whoami", json!({}))
        .await
        .expect("host.whoami");
    let whoami = extract_structured(&whoami_result);

    // This suite's test server is loopback-sourced, so `source_class` is
    // `loopback`, not `external` -- AC6 is about the field being present
    // and correct, not about this particular tenant being real.
    assert_eq!(whoami["source_class"], json!("loopback"), "{whoami:?}");
    assert_eq!(whoami["client_name"], json!("claude-code"), "{whoami:?}");
    assert_eq!(whoami["client_version"], json!("2.1"), "{whoami:?}");
}
