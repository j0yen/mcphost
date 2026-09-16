//! PRD-mcphost-agent-consent
//! AC1 (P0) — Given tenant R has `contact_policy: contacts` and no
//! contacts, When stranger S calls `host.msg.send(to=[R])`, Then `refused`
//! contains R with `contact_refused` and `data.hint` naming
//! `host.agent.contact_request`, and R's inbox is empty.
//! AC2 (P0) — Given S calls `host.agent.contact_request(R, note="reviewing
//! your indexer")`, When R lists `host.agent.contacts(status="pending")`,
//! Then the request appears with S's address and the note; and When S
//! sends again before a decision, Then the refusal is `contact_pending`.
//! AC3 (P0) — Given R calls `host.agent.contact_accept(request_id)`, When S
//! sends R a message, Then it is delivered and both
//! `host.agent.contacts()` results show the pair with `accepted_at` set.
//! AC4 (P0) — Given R denies the request, When S sends or requests again
//! within 7 days, Then both return `contact_pending`; and after the
//! cool-down (backdated directly against the test server's own `Db` handle
//! -- see `Db::test_backdate_contact_request`'s doc comment for why this
//! repo backdates rather than waiting out a real 7 days), Then a new
//! request is accepted as `pending`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn ac1_contacts_policy_stranger_refused_with_hint() {
    let server = TestServer::start().await;
    let (_ns_a, key_a) = signup(&server.base_url, "Agent A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Agent B").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);

    client_b
        .tools_call("host.agent.profile_set", json!({"contact_policy": "contacts"}))
        .await
        .expect("B sets contacts policy");

    let raw = client_a
        .tools_call("host.msg.send", json!({"to": [ns_b.clone()], "body": "ping"}))
        .await
        .expect("call succeeds, recipient is refused");
    let result = extract_structured(&raw);
    assert_eq!(result["delivered_to"], json!([]), "{result:?}");
    let refused = result["refused"].as_array().expect("refused array");
    assert_eq!(refused.len(), 1, "{result:?}");
    assert_eq!(refused[0]["address"], json!(ns_b), "{result:?}");
    assert_eq!(refused[0]["code"], json!("contact_refused"), "{result:?}");
    assert_eq!(
        refused[0]["data"]["hint"],
        json!("host.agent.contact_request"),
        "{result:?}"
    );

    let inbox_raw = client_b.tools_call("host.msg.inbox", json!({})).await.expect("B inbox");
    let inbox = extract_structured(&inbox_raw);
    assert_eq!(inbox["messages"].as_array().unwrap().len(), 0, "{inbox:?}");
}

#[tokio::test]
async fn ac2_contact_request_visible_pending_and_blocks_repeat_send() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "Agent A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Agent B").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);

    client_b
        .tools_call("host.agent.profile_set", json!({"contact_policy": "contacts"}))
        .await
        .expect("B sets contacts policy");

    let raw = client_a
        .tools_call(
            "host.agent.contact_request",
            json!({"address": ns_b.clone(), "note": "reviewing your indexer"}),
        )
        .await
        .expect("A requests contact with B");
    let result = extract_structured(&raw);
    assert_eq!(result["status"], json!("pending"), "{result:?}");

    let contacts_raw = client_b
        .tools_call("host.agent.contacts", json!({"status": "pending"}))
        .await
        .expect("B lists pending requests");
    let contacts = extract_structured(&contacts_raw);
    let incoming = contacts["incoming"].as_array().expect("incoming array");
    assert_eq!(incoming.len(), 1, "{contacts:?}");
    assert_eq!(incoming[0]["address"], json!(ns_a), "{contacts:?}");
    assert_eq!(incoming[0]["note"], json!("reviewing your indexer"), "{contacts:?}");
    assert_eq!(incoming[0]["status"], json!("pending"), "{contacts:?}");

    let send_raw = client_a
        .tools_call("host.msg.send", json!({"to": [ns_b.clone()], "body": "ping"}))
        .await
        .expect("call succeeds, recipient is refused");
    let result = extract_structured(&send_raw);
    let refused = result["refused"].as_array().expect("refused array");
    assert_eq!(refused[0]["code"], json!("contact_pending"), "{result:?}");
}

#[tokio::test]
async fn ac3_accept_delivers_and_shows_the_pair_from_both_sides() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "Agent A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Agent B").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);

    client_b
        .tools_call("host.agent.profile_set", json!({"contact_policy": "contacts"}))
        .await
        .expect("B sets contacts policy");

    let req_raw = client_a
        .tools_call("host.agent.contact_request", json!({"address": ns_b.clone()}))
        .await
        .expect("A requests contact with B");
    let request_id = extract_structured(&req_raw)["request_id"]
        .as_str()
        .expect("request_id")
        .to_string();

    let accept_raw = client_b
        .tools_call("host.agent.contact_accept", json!({"request_id": request_id}))
        .await
        .expect("B accepts");
    let accept = extract_structured(&accept_raw);
    assert_eq!(accept["status"], json!("accepted"), "{accept:?}");
    assert_eq!(accept["address"], json!(ns_a), "{accept:?}");

    let send_raw = client_a
        .tools_call("host.msg.send", json!({"to": [ns_b.clone()], "body": "ping"}))
        .await
        .expect("A sends to B");
    let send = extract_structured(&send_raw);
    assert_eq!(send["delivered_to"], json!([ns_b]), "{send:?}");
    assert_eq!(send["refused"], json!([]), "{send:?}");

    let a_contacts = extract_structured(
        &client_a.tools_call("host.agent.contacts", json!({})).await.expect("A contacts"),
    );
    let a_list = a_contacts["contacts"].as_array().expect("A contacts array");
    assert_eq!(a_list.len(), 1, "{a_contacts:?}");
    assert_eq!(a_list[0]["address"], json!(ns_b), "{a_contacts:?}");
    assert!(a_list[0]["accepted_at"].is_string(), "{a_contacts:?}");

    let b_contacts = extract_structured(
        &client_b.tools_call("host.agent.contacts", json!({})).await.expect("B contacts"),
    );
    let b_list = b_contacts["contacts"].as_array().expect("B contacts array");
    assert_eq!(b_list.len(), 1, "{b_contacts:?}");
    assert_eq!(b_list[0]["address"], json!(ns_a), "{b_contacts:?}");
    assert!(b_list[0]["accepted_at"].is_string(), "{b_contacts:?}");
}

#[tokio::test]
async fn ac4_deny_cooldown_then_reexpiry_allows_a_fresh_request() {
    let server = TestServer::start().await;
    let (_ns_a, key_a) = signup(&server.base_url, "Agent A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Agent B").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);

    client_b
        .tools_call("host.agent.profile_set", json!({"contact_policy": "contacts"}))
        .await
        .expect("B sets contacts policy");

    let req_raw = client_a
        .tools_call("host.agent.contact_request", json!({"address": ns_b.clone()}))
        .await
        .expect("A requests contact with B");
    let request_id = extract_structured(&req_raw)["request_id"]
        .as_str()
        .expect("request_id")
        .to_string();

    let deny_raw = client_b
        .tools_call("host.agent.contact_deny", json!({"request_id": request_id.clone()}))
        .await
        .expect("B denies");
    assert_eq!(extract_structured(&deny_raw)["status"], json!("denied"));

    // Within the 7-day cooldown: both a plain send and a fresh
    // contact_request read as contact_pending (AC4's own wording).
    let send_raw = client_a
        .tools_call("host.msg.send", json!({"to": [ns_b.clone()], "body": "ping"}))
        .await
        .expect("call succeeds, recipient is refused");
    let refused = extract_structured(&send_raw)["refused"].as_array().unwrap().clone();
    assert_eq!(refused[0]["code"], json!("contact_pending"), "{refused:?}");

    let retry_err = client_a
        .tools_call("host.agent.contact_request", json!({"address": ns_b.clone()}))
        .await
        .expect_err("still within cooldown");
    assert_eq!(retry_err.error_code.as_deref(), Some("contact_pending"), "{retry_err:?}");

    // Push decided_unix_ms 8 days into the past so the cooldown has
    // elapsed.
    let eight_days_ms = 8 * 24 * 3_600_000_i64;
    let now_ms = mcphost::state::now_unix_ms();
    server
        .state
        .db
        .test_backdate_contact_request(request_id.clone(), None, Some(now_ms - eight_days_ms))
        .await
        .expect("backdate decided_unix_ms");

    let fresh_raw = client_a
        .tools_call("host.agent.contact_request", json!({"address": ns_b.clone()}))
        .await
        .expect("cooldown elapsed, a fresh request is accepted");
    let fresh = extract_structured(&fresh_raw);
    assert_eq!(fresh["status"], json!("pending"), "{fresh:?}");

    let fresh_id = fresh["request_id"].as_str().expect("fresh request_id").to_string();
    let accept_raw = client_b
        .tools_call("host.agent.contact_accept", json!({"request_id": fresh_id}))
        .await
        .expect("B can accept the fresh request");
    assert_eq!(extract_structured(&accept_raw)["status"], json!("accepted"));
}
