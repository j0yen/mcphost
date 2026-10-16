//! PRD-mcphost-email-attachments-as-blobs
//! AC5 -- Given a part named `report.csv` whose bytes are a PE binary, When
//! received, Then it is `type_not_allowed`, nothing is written to disk, and a
//! part whose declared mime is `text/plain` but sniffs as CSV is stored with
//! `declared_mime` recorded.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use base64::Engine as _;
use mcphost::email_inbound::{InboundMail, InboundPart, message_key, receive};
use serde_json::{Value, json};

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn part(filename: &str, content_type: &str, bytes: &[u8]) -> InboundPart {
    InboundPart { filename: filename.into(), content_type: content_type.into(), content_base64: b64(bytes), inline: false }
}

fn mail(id: &str, parts: Vec<InboundPart>) -> InboundMail {
    InboundMail { provider_message_id: id.into(), from: "boss@example.com".into(), subject: "Subject".into(), parts }
}

async fn inbox_data(client: &McpClient) -> Vec<Value> {
    let inbox = extract_structured(&client.tools_call("host.msg.inbox", json!({})).await.expect("inbox"));
    inbox["messages"].as_array().expect("messages").iter().map(|m| m["data"].clone()).collect()
}


fn files_under(dir: &std::path::Path) -> usize {
    std::fs::read_dir(dir)
        .map(|d| d.flatten().map(|e| if e.path().is_dir() { files_under(&e.path()) } else { 1 }).sum())
        .unwrap_or(0)
}

#[tokio::test]
async fn pe_named_csv_is_type_not_allowed_and_plain_text_csv_keeps_its_declared_mime() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Ematt AC5").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("lookup").expect("tenant");

    let mut pe = b"MZ\x90\x00\x03\x00\x00\x00\x04\x00\x00\x00\xff\xff\x00\x00".to_vec();
    pe.extend_from_slice(&[0u8; 256]);
    pe.extend_from_slice(b"PE\0\0\x4c\x01\x03\x00");
    let csv = b"a,b\n1,2\n3,4\n".to_vec();
    receive(
        &server.state,
        &tenant,
        &mail("prov-ac5", vec![part("report.csv", "text/csv", &pe), part("data.csv", "text/plain", &csv)]),
    )
    .await
    .expect("the mail lands");

    let msgs = inbox_data(&client).await;
    let atts = msgs[0]["attachments"].as_array().expect("attachments");
    assert_eq!(atts[0]["rejected_reason"], "type_not_allowed", "{atts:?}");
    assert!(atts[0].get("blob_id").is_none(), "{atts:?}");
    assert_eq!(atts[1]["mime"], "text/csv", "{atts:?}");
    assert_eq!(atts[1]["declared_mime"], "text/plain", "{atts:?}");
    assert!(atts[1]["blob_id"].as_str().is_some(), "{atts:?}");

    // Only the CSV reached disk; the PE binary was never written.
    let on_disk = files_under(&server.state.db.data_dir().join("blobs"));
    assert_eq!(on_disk, 1, "exactly the CSV is on disk");
    assert_eq!(server.state.db.email_blob_count(tenant.id, None).await.unwrap(), 1);
    let (row, declared, _inline) = server
        .state
        .db
        .blob_find_email_part(tenant.id, message_key("prov-ac5"), 1)
        .await
        .expect("lookup")
        .expect("the CSV part's row");
    assert_eq!(declared.as_deref(), Some("text/plain"), "declared_mime is recorded on the row");
    assert_eq!(row.mime, "text/csv");
}
