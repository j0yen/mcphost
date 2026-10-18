//! PRD-mcphost-self-offboard-releases-session
//! AC2 (P0) — Given tenant A offboarded itself, When its old key calls
//! `host.whoami`, Then the error is `tenant_disabled` with
//! `data.disabled_reason` `self_offboard` and `remedy` containing "call any
//! host.* with no key on a new session", and a repeat `host.self_offboard`
//! still returns `already_offboarded: true`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn sorel_ac02_old_key_is_refused_with_the_fresh_start_remedy() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Sorel Remedy").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client.tools_call("host.self.offboard", json!({})).await.expect("self_offboard");

    let err = client
        .tools_call("host.whoami", json!({}))
        .await
        .expect_err("an offboarded tenant's key must be refused");
    assert_eq!(err.error_code.as_deref(), Some("tenant_disabled"), "{err:?}");
    assert_eq!(err.data["disabled_reason"], json!("self_offboard"), "{err:?}");
    let remedy = err.data["remedy"].as_str().expect("remedy string");
    assert!(
        remedy.contains("call any host.* with no key on a new session"),
        "remedy must name the fresh-start path: {remedy}"
    );

    let again = extract_structured(
        &client.tools_call("host.self.offboard", json!({})).await.expect("repeat self_offboard"),
    );
    assert_eq!(again["already_offboarded"], json!(true), "got {again}");
}
