//! PRD-mcphost-scoped-tool-share
//! AC6 (P0) -- Given `visibility: "url"` with `scopes: ["read_source"]`, When
//! `host.tool_share` runs, Then `args_invalid` says url shares allow only
//! `call`; Given `scopes: ["admin"]`, Then `args_invalid` lists the three
//! valid scopes.

use serde_json::json;

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::sharing::ShareScope;

#[tokio::test]
async fn shsc_ac06_url_share_scope_and_unknown_scope_are_args_invalid() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Owner A").await;
    let owner = McpClient::with_bearer(&server.base_url, &key);
    owner
        .tools_call(
            "host.tool_publish",
            json!({"name": "ping", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");

    let err = owner
        .tools_call("host.tool_share", json!({"name": "ping", "visibility": "url", "scopes": ["read_source"]}))
        .await
        .expect_err("a url share with read_source must be refused");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{}", err.message);
    assert!(err.message.contains("url shares allow only"), "message: {}", err.message);
    assert!(err.message.contains("call"), "message: {}", err.message);

    let err = owner
        .tools_call("host.tool_share", json!({"name": "ping", "visibility": "public", "scopes": ["admin"]}))
        .await
        .expect_err("an unknown scope must be refused");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{}", err.message);
    for scope in ShareScope::ALL {
        assert!(err.message.contains(scope.as_str()), "message must list {}: {}", scope.as_str(), err.message);
    }

    // The llms.txt scopes line is rendered from the same enum: every name is in it.
    let llms = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/www/llms.txt")).expect("llms.txt");
    for scope in ShareScope::ALL {
        assert!(llms.contains(scope.as_str()), "www/llms.txt must mention scope {}", scope.as_str());
    }
}
