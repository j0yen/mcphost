//! AC1 — header AND `tenant_key` argument: `host.self.offboard` returns the
//! success envelope, not `tenant is disabled`.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn soff_ac01_header_and_arg_returns_success_envelope() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Leaver Both").await;
    let client = McpClient::with_bearer(&server.base_url, &key).with_session_continuity();
    client.tools_call("host.whoami", json!({"tenant_key": key})).await.unwrap();
    let out = client
        .tools_call("host.self.offboard", json!({"tenant_key": key}))
        .await
        .expect("header+arg self_offboard must be the success envelope, not an error");
    assert_eq!(out["structuredContent"]["disabled"], json!(true), "got {out}");
    assert_eq!(out["structuredContent"]["disabled_reason"], json!("self_offboard"));
}
