//! PRD-mcphost-email-attachments-as-blobs
//! AC2 -- Given a fake-signed inbound mail with a PDF and a CSV part, When
//! received, Then the inbox message lists two attachments with `blob_id`,
//! `url`, sniffed `mime` and the right `bytes`, and `mcphost.blob.get(blob_id)`
//! inside a python tool returns bytes whose sha256 matches the row.

use crate::common;
use base64::Engine as _;
use common::{McpClient, TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::email_inbound::{InboundMail, InboundPart, receive};
use mcphost::sandbox;
use serde_json::{Value, json};
use std::time::Duration;

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn pdf(len: usize) -> Vec<u8> {
    let mut v = b"%PDF-1.4\n".to_vec();
    v.resize(len, b'x');
    v
}

fn csv(len: usize) -> Vec<u8> {
    let mut v = b"a,b\n1,2\n".to_vec();
    while v.len() < len {
        v.extend_from_slice(b"3,4\n");
    }
    v.truncate(len);
    v
}

#[tokio::test]
async fn pdf_and_csv_parts_land_as_blobs_a_python_tool_reads_by_id() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Ematt AC2").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server.state.db.find_tenant_by_namespace(ns.clone()).await.expect("lookup").expect("tenant");

    let pdf_bytes = pdf(180 * 1024);
    let csv_bytes = csv(4 * 1024);
    let mail = InboundMail {
        provider_message_id: "prov-msg-ac2".into(),
        from: "boss@example.com".into(),
        subject: "Invoice".into(),
        parts: vec![
            InboundPart {
                filename: "invoice.pdf".into(),
                content_type: "application/pdf".into(),
                content_base64: b64(&pdf_bytes),
                inline: false,
            },
            InboundPart {
                filename: "lines.csv".into(),
                content_type: "text/csv".into(),
                content_base64: b64(&csv_bytes),
                inline: false,
            },
        ],
    };
    receive(&server.state, &tenant, &mail).await.expect("receive");

    let inbox = extract_structured(&client.tools_call("host.msg.inbox", json!({})).await.expect("inbox"));
    let messages = inbox["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 1, "{inbox:?}");
    let data = &messages[0]["data"];
    assert_eq!(data["kind"], "email_received");
    let atts = data["attachments"].as_array().expect("attachments");
    assert_eq!(atts.len(), 2, "{data:?}");
    assert_eq!(atts[0]["name"], "invoice.pdf");
    assert_eq!(atts[0]["mime"], "application/pdf");
    assert_eq!(atts[0]["bytes"], pdf_bytes.len());
    assert_eq!(atts[1]["mime"], "text/csv");
    assert_eq!(atts[1]["bytes"], csv_bytes.len());
    for a in atts {
        assert!(a["blob_id"].as_str().is_some_and(|s| !s.is_empty()), "{a:?}");
        let url = a["url"].as_str().expect("url");
        let resp = reqwest::get(url).await.expect("GET url");
        assert_eq!(resp.status(), reqwest::StatusCode::OK, "{a:?}");
    }

    let spec = json!({
        "network": "none",
        "source": "import hashlib, mcphost\ndef main(args):\n    data = mcphost.blob.get(args['blob'])\n    return {\"len\": len(data), \"sha256\": hashlib.sha256(data).hexdigest()}\n",
        "args_schema": {"type": "object", "properties": {"blob": {"type": "string"}}, "required": ["blob"]},
    });
    client
        .tools_call("host.tool_publish", json!({"name": "att_reader", "kind": "python", "spec": spec}))
        .await
        .expect("publish");
    for (att, bytes) in [(&atts[0], &pdf_bytes), (&atts[1], &csv_bytes)] {
        let blob_id: &Value = &att["blob_id"];
        let result =
            poll_until_ready(&client, &format!("{ns}.att_reader"), json!({"blob": blob_id}), Duration::from_secs(15))
                .await
                .unwrap_or_else(|e| panic!("call must succeed: {} {}", e.code, e.message));
        let out = extract_structured(&result);
        assert_eq!(out["len"], bytes.len());
        assert_eq!(out["sha256"], mcphost::billing::sha256_hex(bytes));
        let row = server
            .state
            .db
            .blob_find_by_id(tenant.id, blob_id.as_str().unwrap().to_string())
            .await
            .expect("row lookup")
            .expect("row");
        assert_eq!(row.sha256, out["sha256"].as_str().unwrap(), "tool bytes match the row");
    }
}
