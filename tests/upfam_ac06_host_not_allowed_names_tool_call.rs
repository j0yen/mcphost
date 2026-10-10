//! AC6 — Given a tool spec whose `url` host is the configured own domain,
//! When published or called, Then `host_not_allowed` carries `data.hint`
//! naming `host.tool_call`.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::kinds::KindRegistry;
use mcphost::kinds::http::HttpKind;
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn own_domain_url_is_host_not_allowed_with_tool_call_hint() {
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(HttpKind::new("mcphost.dev").expect("http kind")));
    let server = TestServer::start_with_kinds(kinds).await;
    let (_ns, key) = signup(&server.base_url, "Own Domain Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "method": "POST",
        "url": "https://mcphost.dev/mcp",
        "args_schema": {"type": "object"},
    });
    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "self_call", "kind": "http", "spec": spec}),
        )
        .await
        .expect_err("publishing an own-domain url must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("host_not_allowed"));
    let hint = err.data["hint"].as_str().expect("hint is a string");
    assert!(hint.contains("host.tool_call"), "{hint}");
}
