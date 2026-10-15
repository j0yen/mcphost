//! Shared helpers for the `emin_ac*` tests (PRD-mcphost-inbound-email-address-per-tenant).

#![allow(dead_code)]

use mcphost::email_inbound::ResendInbound;
use serde_json::json;

pub const SECRET: &str = "whsec-test";

pub fn resend_body(to: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "type": "email.received",
        "data": {
            "email_id": "em_4421",
            "from": "ap@vendor.com",
            "to": [to],
            "subject": "Invoice 4421",
            "text": "Please pay 120 EUR.",
            "html": "<p>Please pay <b>120</b> EUR &amp; thanks</p><style>p{color:red}</style>",
            "attachments": [{"filename": "inv.pdf", "content_type": "application/pdf", "size": 2048}],
        }
    }))
    .expect("json")
}

pub async fn post_signed(base: &str, body: &[u8], id: &str, ts: i64, secret: &str) -> reqwest::Response {
    let sig = ResendInbound::new(secret).sign(id, ts, body);
    reqwest::Client::new()
        .post(format!("{base}/email/inbound"))
        .header("svix-id", id)
        .header("svix-timestamp", ts.to_string())
        .header("svix-signature", sig)
        .header("content-type", "application/json")
        .body(body.to_vec())
        .send()
        .await
        .expect("post")
}
