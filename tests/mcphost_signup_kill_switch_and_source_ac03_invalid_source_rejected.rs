//! AC3 (PRD-mcphost-signup-kill-switch-and-source) — Given `source: "HN!"`
//! or a 65-character value, When signup is called, Then `invalid_argument`
//! (this crate's `args_invalid` code -- see `errors.rs`'s doc comment: it
//! is the one code every bad-argument rejection in this crate uses) names
//! `source` and no tenant is created.

use crate::common;
use common::{McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn invalid_source_is_rejected_and_creates_no_tenant() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let before = server.state.db.list_tenants().await.expect("list tenants");
    assert_eq!(before.len(), 0);

    let err = client
        .tools_call("signup", json!({"name": "Bad Punctuation", "source": "HN!"}))
        .await
        .expect_err("uppercase/punctuation source must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{err:?}");
    assert_eq!(err.data["field"], json!("source"), "{err:?}");

    let too_long = "a".repeat(65);
    let err = client
        .tools_call("signup", json!({"name": "Too Long", "source": too_long}))
        .await
        .expect_err("65-char source must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{err:?}");
    assert_eq!(err.data["field"], json!("source"), "{err:?}");

    let after = server.state.db.list_tenants().await.expect("list tenants");
    assert_eq!(after.len(), 0, "no tenant must be created on a rejected source");
}
