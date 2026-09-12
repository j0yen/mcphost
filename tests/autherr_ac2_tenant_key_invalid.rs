//! PRD-mcphost-auth-error-names-argument
//! AC2 (P0) — Given `tenant_key: "t_notreal"`, When any `host.*` tool is
//! called, Then the error has `error_code` `tenant_key_invalid` and the
//! message does not contain `t_notreal`.

use crate::common;
use common::{McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn unrecognized_tenant_key_never_echoes_the_key() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let err = client
        .tools_call("host.tool_list", json!({"tenant_key": "t_notreal"}))
        .await
        .expect_err("an unrecognized tenant_key must be refused");

    assert_eq!(err.error_code.as_deref(), Some("tenant_key_invalid"));
    assert!(
        !err.message.contains("t_notreal"),
        "message must never echo the offending key: {}",
        err.message
    );
    assert_eq!(
        err.message,
        "tenant_key was not recognized; call signup for a new key or check the value"
    );
}

#[tokio::test]
async fn unrecognized_tenant_key_is_refused_the_same_way_across_host_tools() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    for tool in ["host.whoami", "host.usage", "host.tool_call"] {
        let result = client.tools_call(tool, json!({"tenant_key": "t_notreal"})).await;
        let err = match result {
            Ok(_) => panic!("{tool} with an unrecognized tenant_key must be refused"),
            Err(e) => e,
        };
        assert_eq!(
            err.error_code.as_deref(),
            Some("tenant_key_invalid"),
            "{tool} did not report tenant_key_invalid"
        );
    }
}
