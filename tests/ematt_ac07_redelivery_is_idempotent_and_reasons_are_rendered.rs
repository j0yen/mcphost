//! PRD-mcphost-email-attachments-as-blobs
//! AC7 -- Given the same provider message delivered twice, When handled, Then
//! exactly one blob per part exists; Given the `RejectedReason` enum, Then the
//! drift test finds every variant, including `store_unavailable`, in the
//! rendered description.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use base64::Engine as _;
use mcphost::email_inbound::{InboundMail, InboundPart, RejectedReason, receive};
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
async fn the_same_provider_message_twice_stores_one_blob_per_part_and_one_message() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Ematt AC7 dup").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("lookup").expect("tenant");

    let m = mail(
        "prov-ac7",
        vec![part("a.csv", "text/csv", b"a,b\n1,2\n"), part("b.csv", "text/csv", b"c,d\n3,4\n")],
    );
    let first = receive(&server.state, &tenant, &m).await.expect("first delivery");
    let second = receive(&server.state, &tenant, &m).await.expect("redelivery");

    assert_eq!(first["created"], true);
    assert_eq!(second["created"], false, "{second:?}");
    assert_eq!(first["message_id"], second["message_id"]);
    assert_eq!(server.state.db.email_blob_count(tenant.id, None).await.unwrap(), 2, "one blob per part");
    assert_eq!(
        first["data"]["attachments"][0]["blob_id"], second["data"]["attachments"][0]["blob_id"],
        "the redelivery reports the blobs the first delivery stored"
    );
    assert_eq!(inbox_data(&client).await.len(), 1, "one inbox message");
}

/// Exhaustive on purpose: a new variant fails to compile here until it is
/// listed, and then the description check below covers it.
fn variant_name(r: RejectedReason) -> &'static str {
    match r {
        RejectedReason::TooLarge => "too_large",
        RejectedReason::QuotaExceeded => "quota_exceeded",
        RejectedReason::TypeNotAllowed => "type_not_allowed",
        RejectedReason::DecodeFailed => "decode_failed",
        RejectedReason::TooManyParts => "too_many_parts",
        RejectedReason::StoreUnavailable => "store_unavailable",
    }
}

#[tokio::test]
async fn every_rejected_reason_variant_is_in_the_rendered_inbox_and_wait_descriptions() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Ematt AC7 drift").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tools = client.tools_list().await.expect("tools/list");
    let tools = tools["tools"].as_array().expect("tools");

    assert_eq!(RejectedReason::ALL.len(), 6);
    assert!(RejectedReason::ALL.contains(&RejectedReason::StoreUnavailable));
    for verb in ["host.msg.inbox", "host.msg.wait"] {
        let tool = tools.iter().find(|t| t["name"] == verb).unwrap_or_else(|| panic!("{verb} listed"));
        let desc = tool["description"].as_str().expect("description");
        for r in RejectedReason::ALL {
            assert_eq!(r.as_str(), variant_name(*r));
            assert!(desc.contains(r.as_str()), "{verb} description lacks {}: {desc}", r.as_str());
        }
    }
}
