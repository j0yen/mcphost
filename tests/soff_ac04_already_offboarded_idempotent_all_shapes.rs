//! AC4 — a tenant already self-offboarded receives `{disabled: true,
//! already_offboarded: true}` on a second call, for every auth shape
//! (header, argument, both).  The URL shape is covered by `soff_matrix`
//! (AC5) which also includes the URL secret path.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn soff_ac04_already_offboarded_idempotent_header() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Idempotent Header").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.self.offboard", json!({}))
        .await
        .expect("first self_offboard via header must succeed");

    let again = client
        .tools_call("host.self.offboard", json!({}))
        .await
        .expect("second self_offboard via header must be the already_offboarded envelope, not an error");
    let again = extract_structured(&again);
    assert_eq!(again["disabled"], json!(true), "got {again}");
    assert_eq!(again["already_offboarded"], json!(true), "got {again}");
}

#[tokio::test]
async fn soff_ac04_already_offboarded_idempotent_arg() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Idempotent Arg").await;
    let client = McpClient::new(&server.base_url);

    client
        .tools_call("host.self.offboard", json!({"tenant_key": key}))
        .await
        .expect("first self_offboard via arg must succeed");

    let again = client
        .tools_call("host.self.offboard", json!({"tenant_key": key}))
        .await
        .expect("second self_offboard via arg must be the already_offboarded envelope, not an error");
    let again = extract_structured(&again);
    assert_eq!(again["disabled"], json!(true), "got {again}");
    assert_eq!(again["already_offboarded"], json!(true), "got {again}");
}

#[tokio::test]
async fn soff_ac04_already_offboarded_idempotent_both() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Idempotent Both").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.self.offboard", json!({"tenant_key": key}))
        .await
        .expect("first self_offboard via header+arg must succeed");

    let again = client
        .tools_call("host.self.offboard", json!({"tenant_key": key}))
        .await
        .expect("second self_offboard via header+arg must be the already_offboarded envelope, not an error");
    let again = extract_structured(&again);
    assert_eq!(again["disabled"], json!(true), "got {again}");
    assert_eq!(again["already_offboarded"], json!(true), "got {again}");
}
