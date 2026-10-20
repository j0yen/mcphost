//! AC4 — Given spec url `https://<own_domain>/api/x`, When tested, Then the
//! error and hint are the pre-change `host_not_allowed` row.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::kinds::KindRegistry;
use mcphost::kinds::http::HttpKind;
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn ohur_ac04() {
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(HttpKind::new("mcphost.dev").expect("http kind")));
    let server = TestServer::start_with_kinds(kinds).await;
    let (_ns, key) = signup(&server.base_url, "OHUR A").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let spec = json!({"method": "POST", "url": "https://mcphost.dev/api/x"});
    let err = client
        .tools_call("host.bridge_test", json!({"spec": spec, "args": {}}))
        .await
        .expect_err("own-domain url must be refused");
    assert_eq!(err.error_code.as_deref(), Some("host_not_allowed"), "{err:?}");
    assert_eq!(
        err.data["hint"].as_str(),
        Some("call your own tools with host.tool_call; the http kind targets third-party APIs")
    );
    assert!(err.data.get("trigger").is_none(), "{:?}", err.data);
}
