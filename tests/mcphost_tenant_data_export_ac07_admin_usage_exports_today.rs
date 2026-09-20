//! PRD-mcphost-tenant-data-export
//! AC7 (P2) -- Given `admin.usage`, When read, Then `exports_today` is
//! present.

use serde_json::json;

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};

#[tokio::test]
async fn admin_usage_reports_exports_today() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Export AC7 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.export", json!({}))
        .await
        .expect("host.export ok");

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let admin_usage = extract_structured(
        &admin
            .tools_call("admin.usage", json!({"window": "24h"}))
            .await
            .expect("admin.usage ok"),
    );
    let exports_today = admin_usage
        .get("exports_today")
        .unwrap_or_else(|| panic!("admin.usage missing exports_today: {admin_usage:?}"));
    assert!(
        exports_today.as_i64().expect("exports_today is an integer") >= 1,
        "exports_today: {exports_today:?}"
    );
}
