//! PRD-mcphost-inbound-email-address-per-tenant
//! AC3 -- Given a Resend-shaped POST with a valid `svix-signature` for the
//! tenant's address, When `/email/inbound` handles it, Then one inbox message
//! with `data.kind == "email_received"`, the parsed `from`, `subject`, `text`
//! and `html_stripped` exists and `host.msg.wait` returns it within 10 s.

use crate::common;

use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::email::EmailProvider;
use crate::emin;
use emin::{SECRET, post_signed, resend_body};
use serde_json::json;

#[tokio::test]
async fn emin_ac03_signed_mail_is_one_email_received_message() {
    let server = TestServer::start_with_email_inbound(SECRET, EmailProvider::Resend, false).await;
    let (ns, key) = signup(&server.base_url, "emin tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let addr = extract_structured(&client.tools_call("host.email.address", json!({})).await.expect("address"));
    let body = resend_body(addr["address"].as_str().expect("address"));

    let resp = post_signed(&server.base_url, &body, "msg_1", mcphost::state::now_unix(), SECRET).await;
    assert_eq!(resp.status(), 200, "{}", resp.text().await.unwrap_or_default());

    let wait = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        client.tools_call("host.msg.wait", json!({"timeout_s": 5})),
    )
    .await
    .expect("host.msg.wait within 10 s")
    .map(|v| extract_structured(&v))
    .expect("msg.wait");
    let messages = wait["messages"].as_array().unwrap_or_else(|| panic!("messages shape: {wait}"));
    assert_eq!(messages.len(), 1, "{wait}");
    let data = &messages[0]["data"];
    assert_eq!(data["kind"], json!("email_received"), "{wait}");
    assert_eq!(data["from"], json!("ap@vendor.com"), "{wait}");
    assert_eq!(data["subject"], json!("Invoice 4421"), "{wait}");
    assert_eq!(data["text"], json!("Please pay 120 EUR."), "{wait}");
    assert_eq!(data["html_stripped"], json!("Please pay 120 EUR & thanks"), "{wait}");
    assert_eq!(data["message_id"], json!("em_4421"), "{wait}");
    assert_eq!(data["truncated"], json!(false), "{wait}");
    assert_eq!(data["attachments"], json!([{"name": "inv.pdf", "mime": "application/pdf", "bytes": 2048}]), "{wait}");
    assert_eq!(messages[0]["from_address"], json!("host.email"), "{wait}");

    let log = server.state.db.email_inbound_log_rows().await.expect("log");
    assert_eq!(log, vec![("em_4421".to_string(), "accepted".to_string())], "ns {ns}");
}

#[tokio::test]
async fn emin_ac03_unknown_recipient_is_200_with_a_log_row_and_no_inbox_write() {
    let server = TestServer::start_with_email_inbound(SECRET, EmailProvider::Resend, false).await;
    let (_ns, key) = signup(&server.base_url, "emin tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let body = resend_body("nobody-0000@in.mcphost.dev");
    let resp = post_signed(&server.base_url, &body, "msg_2", mcphost::state::now_unix(), SECRET).await;
    assert_eq!(resp.status(), 200);
    let inbox = extract_structured(&client.tools_call("host.msg.inbox", json!({})).await.expect("inbox"));
    assert_eq!(inbox["messages"].as_array().map(Vec::len), Some(0), "{inbox}");
    let log = server.state.db.email_inbound_log_rows().await.expect("log");
    assert_eq!(log, vec![("em_4421".to_string(), "unknown_recipient".to_string())]);
}
