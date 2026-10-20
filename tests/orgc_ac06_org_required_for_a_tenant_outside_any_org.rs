//! PRD-mcphost-org-cost-attribution
//! AC6 — Given a tenant outside any org, When it calls `host.org.usage`,
//! Then `org_required` with the `host.org.create` hint.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn orgc_ac06() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Loner").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let err = client.tools_call("host.org.usage", json!({})).await.expect_err("no org");
    assert_eq!(err.error_code.as_deref(), Some("org_required"), "{err:?}");
    assert_eq!(err.data["next_call"], json!("host.org.create"), "{:?}", err.data);
    assert!(err.data["hint"].as_str().is_some_and(|h| h.contains("host.org.create")), "{:?}", err.data);
}
