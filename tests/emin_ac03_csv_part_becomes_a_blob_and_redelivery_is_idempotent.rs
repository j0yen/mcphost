//! PRD-mcphost-email-inbound-route
//! AC3 -- Given a known namespace and an authorized caller, When a payload
//! with a 4 KB CSV part is posted, Then 200 with `receive()`'s message JSON
//! whose `attachments[0]` carries `blob_id`, `url`, `mime: "text/csv"`,
//! `bytes: 4096`, and `host.msg.inbox` lists the message; posting the same
//! `provider_message_id` again returns the same message id and adds no blob.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use base64::Engine as _;
use serde_json::{Value, json};

#[tokio::test]
async fn emin_ac03_csv_part_becomes_a_blob_and_redelivery_is_idempotent() {
    let server = TestServer::start_with_signup_rate_limit_and_fleet_ips(100, "127.0.0.1").await;
    let (ns, key) = signup(&server.base_url, "Emin AC3").await;
    let tenant = server.state.db.find_tenant_by_namespace(ns.clone()).await.expect("lookup").expect("tenant");
    let mut csv = String::from("a,b\n");
    while csv.len() < 4096 {
        csv.push_str("1,2\n");
    }
    csv.truncate(4096);
    let payload = json!({
        "provider_message_id": "prov-emin-ac3",
        "from": "probe@example.com",
        "subject": "lines.csv",
        "parts": [{
            "filename": "lines.csv",
            "content_type": "text/csv",
            "content_base64": base64::engine::general_purpose::STANDARD.encode(csv.as_bytes()),
            "inline": false,
        }],
    });
    let http = reqwest::Client::new();
    let url = format!("{}/email/inbound/{ns}", server.base_url);

    let resp = http.post(&url).json(&payload).send().await.expect("POST");
    assert_eq!(resp.status(), 200);
    let first: Value = resp.json().await.expect("message json");
    let att = &first["attachments"][0];
    assert!(att["blob_id"].as_str().is_some_and(|b| !b.is_empty()), "{first}");
    assert!(att["url"].as_str().is_some_and(|u| !u.is_empty()), "{first}");
    assert_eq!(att["mime"], json!("text/csv"));
    assert_eq!(att["bytes"], json!(4096));
    assert_eq!(first["data"]["attachments"][0]["blob_id"], att["blob_id"]);
    assert_eq!(server.state.db.email_blob_count(tenant.id, None).await.unwrap(), 1);

    let client = McpClient::with_bearer(&server.base_url, &key);
    let inbox = extract_structured(&client.tools_call("host.msg.inbox", json!({})).await.expect("inbox"));
    let messages = inbox["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 1, "{inbox}");
    assert_eq!(messages[0]["data"]["attachments"][0]["blob_id"], att["blob_id"]);

    let again: Value = http.post(&url).json(&payload).send().await.expect("repost").json().await.expect("json");
    assert_eq!(again["message_id"], first["message_id"], "same provider id, same message");
    assert_eq!(server.state.db.email_blob_count(tenant.id, None).await.unwrap(), 1, "no new blob row");
}
