//! PRD-mcphost-email-attachments-as-blobs
//! AC3 -- Given a part larger than `blob_max_bytes`, When received, Then its
//! entry reads `rejected_reason: too_large` with `bytes`, the other parts are
//! stored, and the message still lands.

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
async fn oversized_part_reads_too_large_and_the_rest_are_stored() {
    let server = TestServer::start_with_plans(|catalog| {
        for p in &mut catalog.plans {
            if p.name == "free" {
                p.blob_max_bytes = 1000;
            }
        }
    })
    .await;
    let (ns, key) = signup(&server.base_url, "Ematt AC3").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("lookup").expect("tenant");

    let big = vec![b'x'; 4096];
    let small = b"a,b\n1,2\n".to_vec();
    receive(
        &server.state,
        &tenant,
        &mail("prov-ac3", vec![part("big.txt", "text/plain", &big), part("small.csv", "text/csv", &small)]),
    )
    .await
    .expect("the mail lands");

    let msgs = inbox_data(&client).await;
    assert_eq!(msgs.len(), 1, "the message still lands: {msgs:?}");
    let atts = msgs[0]["attachments"].as_array().expect("attachments");
    assert_eq!(atts[0]["rejected_reason"], "too_large", "{atts:?}");
    assert_eq!(atts[0]["bytes"], 4096);
    assert!(atts[0].get("blob_id").is_none(), "{atts:?}");
    assert!(atts[1]["blob_id"].as_str().is_some(), "the other part is stored: {atts:?}");
    assert_eq!(server.state.db.email_blob_count(tenant.id, None).await.unwrap(), 1);
}
