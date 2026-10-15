//! PRD-mcphost-inbound-email-address-per-tenant
//! AC4 -- Given a POST with a wrong signature, a timestamp older than 5
//! minutes, or no signature, When handled, Then the response is 401
//! `email_signature_invalid` and no inbox or log row is written.

use crate::common;
use crate::emin;
use emin::{SECRET, post_signed, resend_body};

use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::email::EmailProvider;
use serde_json::{Value, json};

async fn assert_refused(resp: reqwest::Response, what: &str) {
    assert_eq!(resp.status(), 401, "{what}");
    let v: Value = resp.json().await.expect("json body");
    assert_eq!(v["error_code"], json!("email_signature_invalid"), "{what}: {v}");
}

#[tokio::test]
async fn emin_ac04_wrong_stale_or_missing_signature_is_401_and_writes_nothing() {
    let server = TestServer::start_with_email_inbound(SECRET, EmailProvider::Resend, false).await;
    let (_ns, key) = signup(&server.base_url, "emin tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let addr = extract_structured(&client.tools_call("host.email.address", json!({})).await.expect("address"));
    let body = resend_body(addr["address"].as_str().expect("address"));
    let now = mcphost::state::now_unix();

    assert_refused(post_signed(&server.base_url, &body, "msg_a", now, "not-the-secret").await, "wrong secret").await;
    assert_refused(post_signed(&server.base_url, &body, "msg_b", now - 301, SECRET).await, "stale timestamp").await;
    let unsigned = reqwest::Client::new()
        .post(format!("{}/email/inbound", server.base_url))
        .body(body.clone())
        .send()
        .await
        .expect("post");
    assert_refused(unsigned, "no signature").await;

    let inbox = extract_structured(&client.tools_call("host.msg.inbox", json!({})).await.expect("inbox"));
    assert_eq!(inbox["messages"].as_array().map(Vec::len), Some(0), "no inbox row: {inbox}");
    let log = server.state.db.email_inbound_log_rows().await.expect("log");
    assert!(log.is_empty(), "no log row: {log:?}");
}

#[tokio::test]
async fn emin_ac04_route_is_503_without_a_secret() {
    let server = TestServer::start().await;
    let resp = reqwest::Client::new()
        .post(format!("{}/email/inbound", server.base_url))
        .body("{}")
        .send()
        .await
        .expect("post");
    assert_eq!(resp.status(), 503);
    let v: Value = resp.json().await.expect("json");
    assert_eq!(v["error_code"], json!("email_inbound_disabled"), "{v}");
}
