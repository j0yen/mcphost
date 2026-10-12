//! PRD-mcphost-feedback-tool-and-ledger AC5 — Given a keyed tenant that made
//! 7 host calls then sends feedback, When the row is read, Then
//! `last_call_ids` holds the 5 most recent call ids in order and
//! `host.feedback.list` returns the row first with `status: "received"`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

#[tokio::test]
async fn last_call_ids_are_the_five_most_recent_in_order_and_list_returns_it_first() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "fbtl-keyed").await;
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.unwrap().expect("tenant");
    let ring_key = mcphost::feedback::ring_key_tenant(tenant.id);
    let client = McpClient::with_bearer(&server.base_url, &key);

    for _ in 0..7 {
        client.tools_call("host.whoami", json!({})).await.expect("host call");
    }
    let at_send = server.state.feedback_calls.recent(&ring_key);
    assert_eq!(at_send.len(), 5, "7 calls keep only the 5 most recent: {at_send:?}");

    let res = client
        .tools_call("host.feedback.send", json!({"kind": "wrong_error", "text": "first"}))
        .await
        .expect("keyed send");
    let id = extract_structured(&res)["feedback_id"].as_str().unwrap().to_string();
    let row = server.state.db.get_feedback(id.clone()).await.unwrap().expect("row");
    assert_eq!(row.tenant_id, Some(tenant.id));
    let stored: Vec<String> = serde_json::from_str(&row.last_call_ids).expect("json array");
    assert_eq!(stored, at_send, "the row holds exactly the ring as it stood at send time");

    // Order: one more host call drops the oldest id and appends a new one.
    client.tools_call("host.whoami", json!({})).await.unwrap();
    let later = server.state.feedback_calls.recent(&ring_key);
    assert_eq!(&later[..4], &at_send[1..], "oldest first: the window slid by exactly one");
    assert_ne!(later[4], at_send[4]);

    // A second send; list returns it first, then the first, both `received`.
    client
        .tools_call("host.feedback.send", json!({"kind": "praise", "text": "second"}))
        .await
        .unwrap();
    let listed = extract_structured(&client.tools_call("host.feedback.list", json!({})).await.unwrap());
    let rows = listed["feedback"].as_array().expect("feedback array");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["text"], "second", "newest first");
    assert_eq!(rows[1]["feedback_id"], Value::String(id));
    assert!(rows.iter().all(|r| r["status"] == "received"));
    assert!(rows.iter().all(|r| r.get("ip_hash").is_none()));
}

#[tokio::test]
async fn anonymous_list_is_empty_with_a_signup_note() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);
    client
        .tools_call("host.feedback.send", json!({"kind": "bug", "text": "x"}))
        .await
        .unwrap();
    let listed = extract_structured(&client.tools_call("host.feedback.list", json!({})).await.unwrap());
    assert_eq!(listed["feedback"], json!([]));
    assert_eq!(listed["note"], "sign up to list your feedback");
}
