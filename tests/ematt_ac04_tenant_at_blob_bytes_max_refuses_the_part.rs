//! PRD-mcphost-email-attachments-as-blobs
//! AC4 -- Given a tenant at its `blob_bytes_max`, When a mail with a 1 MB part
//! arrives, Then the entry reads `quota_exceeded`, `data.quota` is on the
//! message, and no blob row is written.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use base64::Engine as _;
use mcphost::email_inbound::{InboundMail, InboundPart, receive};
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


#[tokio::test]
async fn part_past_blob_bytes_max_reads_quota_exceeded_with_data_quota() {
    let server = TestServer::start_with_plans(|catalog| {
        for p in &mut catalog.plans {
            if p.name == "free" {
                p.blob_bytes_max = 100;
            }
        }
    })
    .await;
    let (ns, key) = signup(&server.base_url, "Ematt AC4").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("lookup").expect("tenant");

    client
        .tools_call(
            "host.blob.put",
            json!({"name": "fill.bin", "mime": "application/octet-stream", "content_base64": b64(&[7u8; 100])}),
        )
        .await
        .expect("fill the quota exactly");
    let before = server.state.db.blobs_usage(tenant.id).await.expect("usage");

    let mut one_mb = b"%PDF-1.4\n".to_vec();
    one_mb.resize(1024 * 1024, b'x');
    receive(&server.state, &tenant, &mail("prov-ac4", vec![part("big.pdf", "application/pdf", &one_mb)]))
        .await
        .expect("the mail lands");

    let msgs = inbox_data(&client).await;
    assert_eq!(msgs.len(), 1, "{msgs:?}");
    let entry = &msgs[0]["attachments"][0];
    assert_eq!(entry["rejected_reason"], "quota_exceeded", "{entry:?}");
    assert!(entry.get("blob_id").is_none(), "{entry:?}");
    let quota = &msgs[0]["quota"];
    assert_eq!(quota["blob_bytes_max"], 100, "{msgs:?}");
    assert_eq!(quota["blob_bytes"], 100, "{msgs:?}");
    assert!(quota["blobs_max"].is_i64() && quota["blobs"] == 1, "{msgs:?}");

    assert_eq!(server.state.db.blobs_usage(tenant.id).await.expect("usage"), before, "no blob row is written");
    assert_eq!(server.state.db.email_blob_count(tenant.id, None).await.unwrap(), 0);
}
