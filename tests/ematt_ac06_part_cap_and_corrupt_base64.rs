//! PRD-mcphost-email-attachments-as-blobs
//! AC6 -- Given a mail with 25 parts, When received, Then 20 are decided
//! normally and 5 read `too_many_parts`; Given a part whose base64 is corrupt,
//! Then it reads `decode_failed` and the rest are unaffected.

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


fn csv_part(i: usize) -> InboundPart {
    part(&format!("p{i}.csv"), "text/csv", format!("a,b\n{i},{i}\n").as_bytes())
}

#[tokio::test]
async fn twenty_five_parts_decide_twenty_and_refuse_five_as_too_many() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Ematt AC6 many").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("lookup").expect("tenant");

    let parts: Vec<InboundPart> = (0..25).map(csv_part).collect();
    receive(&server.state, &tenant, &mail("prov-ac6-many", parts)).await.expect("the mail lands");

    let msgs = inbox_data(&client).await;
    let atts = msgs[0]["attachments"].as_array().expect("attachments");
    assert_eq!(atts.len(), 25);
    let stored = atts.iter().filter(|a| a.get("blob_id").is_some()).count();
    let too_many = atts.iter().filter(|a| a["rejected_reason"] == "too_many_parts").count();
    assert_eq!((stored, too_many), (20, 5), "{atts:?}");
    assert!(atts[..20].iter().all(|a| a.get("blob_id").is_some()), "the first twenty are decided normally");
    assert_eq!(server.state.db.email_blob_count(tenant.id, None).await.unwrap(), 20);
}

#[tokio::test]
async fn a_corrupt_base64_part_reads_decode_failed_and_the_rest_are_unaffected() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Ematt AC6 corrupt").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("lookup").expect("tenant");

    let corrupt = InboundPart {
        filename: "broken.csv".into(),
        content_type: "text/csv".into(),
        content_base64: "!!! not base64 ???".into(),
        inline: false,
    };
    receive(&server.state, &tenant, &mail("prov-ac6-bad", vec![csv_part(1), corrupt, csv_part(2)]))
        .await
        .expect("the mail lands");

    let msgs = inbox_data(&client).await;
    let atts = msgs[0]["attachments"].as_array().expect("attachments");
    assert_eq!(atts[1]["rejected_reason"], "decode_failed", "{atts:?}");
    assert!(atts[0]["blob_id"].as_str().is_some() && atts[2]["blob_id"].as_str().is_some(), "{atts:?}");
    assert_eq!(server.state.db.email_blob_count(tenant.id, None).await.unwrap(), 2);
}
