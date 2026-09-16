//! PRD-mcphost-agent-consent
//! AC5 (P0, partial -- see the PRD's own deferred-AC note: trigger-firing/
//! `host.msg.wait` don't exist in this codebase yet, PRD-mcphost-agent-wake
//! is a separate, not-yet-built PRD; only the mechanically-implementable
//! clauses are tested here) — Given R muted S, When S sends a non-urgent
//! message, Then it is stored, visible in `host.msg.thread`, absent from
//! `inbox(unread_only=true)`; and When S sends with `urgent=true`, Then the
//! envelope has `urgent: true` and it IS present/counted in
//! `inbox(unread_only=true)` despite the mute.
//! AC6 (P0) — Given the free plan's `urgent_per_day` is 3, When S sends R
//! four urgent messages in a day, Then the fourth returns `quota_exceeded`
//! with `data.limit="urgent_per_day"` and `data.value=3`, and is not
//! stored.
//! AC7 (P0) — Given R blocked S, When S calls
//! `host.agent.contact_request(R)` or sends `urgent=true`, Then both
//! responses are byte-identical (same code) to the responses for a
//! nonexistent address, and any prior pending request from S is denied
//! without notifying S.
//! AC8 (P0) — Given R is `closed`, When any tenant sends `urgent=true`,
//! Then the refusal is `contact_refused` and nothing stored.
//! AC9 (P0) — Given S has a pending request to R and S is deleted via
//! `admin.tenant_delete`, When R lists contacts, Then no request or
//! contact row references S.
//! AC10 (P1) — Given a pending request to R, When R reads
//! `host.msg.inbox()`, Then a system message with `from_address="host"`,
//! `data.kind="contact_request"` and the `request_id` is present, and
//! `host.agent.contact_accept` with that id succeeds.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn ac5_mute_filters_unread_inbox_but_urgent_bypasses_it() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "Agent A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Agent B").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);

    client_b
        .tools_call("host.agent.mute", json!({"address": ns_a.clone()}))
        .await
        .expect("B mutes A");

    let m1_raw = client_a
        .tools_call("host.msg.send", json!({"to": [ns_b.clone()], "body": "m1 non-urgent"}))
        .await
        .expect("A sends non-urgent m1");
    let m1 = extract_structured(&m1_raw);
    assert_eq!(m1["delivered_to"], json!([ns_b]), "{m1:?}");
    let thread_id = m1["thread_id"].as_str().expect("thread_id").to_string();
    let m1_id = m1["message_id"].as_str().expect("m1 message_id").to_string();

    // Stored and visible via host.msg.thread (no change there).
    let thread_raw = client_b
        .tools_call("host.msg.thread", json!({"thread_id": thread_id.clone()}))
        .await
        .expect("B reads the thread");
    let thread = extract_structured(&thread_raw);
    let thread_messages = thread["messages"].as_array().expect("messages array");
    assert_eq!(thread_messages.len(), 1, "{thread:?}");
    assert_eq!(thread_messages[0]["message_id"], json!(m1_id), "{thread:?}");

    // Absent from inbox(unread_only=true).
    let unread_raw = client_b
        .tools_call("host.msg.inbox", json!({"unread_only": true}))
        .await
        .expect("B unread inbox");
    let unread = extract_structured(&unread_raw);
    assert_eq!(unread["messages"].as_array().unwrap().len(), 0, "{unread:?}");

    // A second, urgent message: envelope carries urgent: true, and IS
    // present/counted in inbox(unread_only=true) despite the mute
    // (requirement 6: urgent bypasses the mute filter).
    let m2_raw = client_a
        .tools_call(
            "host.msg.send",
            json!({"to": [ns_b.clone()], "body": "m2 urgent", "thread_id": thread_id, "urgent": true}),
        )
        .await
        .expect("A sends urgent m2");
    let m2 = extract_structured(&m2_raw);
    assert_eq!(m2["delivered_to"], json!([ns_b]), "{m2:?}");
    let m2_id = m2["message_id"].as_str().expect("m2 message_id").to_string();

    let unread_raw2 = client_b
        .tools_call("host.msg.inbox", json!({"unread_only": true}))
        .await
        .expect("B unread inbox after urgent send");
    let unread2 = extract_structured(&unread_raw2);
    let unread2_messages = unread2["messages"].as_array().expect("messages array");
    assert_eq!(unread2_messages.len(), 1, "{unread2:?}");
    assert_eq!(unread2_messages[0]["message_id"], json!(m2_id), "{unread2:?}");
    assert_eq!(unread2_messages[0]["urgent"], json!(true), "{unread2:?}");
}

#[tokio::test]
async fn ac6_urgent_per_day_quota_blocks_the_fourth_send() {
    let server = TestServer::start().await;
    let (_ns_a, key_a) = signup(&server.base_url, "Agent A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Agent B").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);

    // B stays open (default contact_policy) -- urgent is allowed to open
    // recipients with no contact relationship needed.
    for n in 0..3 {
        let raw = client_a
            .tools_call(
                "host.msg.send",
                json!({"to": [ns_b.clone()], "body": format!("urgent {n}"), "urgent": true}),
            )
            .await
            .unwrap_or_else(|e| panic!("urgent send {n} should succeed: {e:?}"));
        let result = extract_structured(&raw);
        assert_eq!(result["delivered_to"], json!([ns_b]), "{result:?}");
    }

    let err = client_a
        .tools_call(
            "host.msg.send",
            json!({"to": [ns_b.clone()], "body": "one too many", "urgent": true}),
        )
        .await
        .expect_err("the 4th urgent send must be refused");
    assert_eq!(err.error_code.as_deref(), Some("quota_exceeded"), "{err:?}");
    assert_eq!(err.data["limit"], json!("urgent_per_day"), "{err:?}");
    assert_eq!(err.data["value"], json!(3), "{err:?}");

    // Not stored: B's inbox still holds exactly the first 3.
    let inbox_raw = client_b.tools_call("host.msg.inbox", json!({"limit": 100})).await.expect("B inbox");
    let inbox = extract_structured(&inbox_raw);
    assert_eq!(inbox["messages"].as_array().unwrap().len(), 3, "{inbox:?}");
}

#[tokio::test]
async fn ac7_block_reads_identically_to_nonexistent_and_denies_the_pending_request() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "Agent A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Agent B").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);
    let nonexistent = "t_doesnotexist00000000".to_string();

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

    client_b
        .tools_call("host.msg.block", json!({"address": ns_a.clone()}))
        .await
        .expect("B blocks A");

    // contact_request against the blocked recipient reads identically to a
    // nonexistent address.
    let blocked_err = client_a
        .tools_call("host.agent.contact_request", json!({"address": ns_b.clone()}))
        .await
        .expect_err("blocked recipient");
    let nonexistent_err = client_a
        .tools_call("host.agent.contact_request", json!({"address": nonexistent.clone()}))
        .await
        .expect_err("nonexistent address");
    assert_eq!(blocked_err.error_code, nonexistent_err.error_code, "{blocked_err:?} vs {nonexistent_err:?}");
    assert_eq!(blocked_err.error_code.as_deref(), Some("agent_not_found"), "{blocked_err:?}");

    // Same for an urgent send.
    let blocked_send = client_a
        .tools_call(
            "host.msg.send",
            json!({"to": [ns_b.clone()], "body": "ping", "urgent": true}),
        )
        .await
        .expect("call itself succeeds, recipient is refused");
    let blocked_refused = extract_structured(&blocked_send)["refused"].as_array().unwrap().clone();
    let nonexistent_send = client_a
        .tools_call(
            "host.msg.send",
            json!({"to": [nonexistent.clone()], "body": "ping", "urgent": true}),
        )
        .await
        .expect("call itself succeeds, recipient is refused");
    let nonexistent_refused = extract_structured(&nonexistent_send)["refused"].as_array().unwrap().clone();
    assert_eq!(blocked_refused[0]["code"], nonexistent_refused[0]["code"], "{blocked_refused:?}");
    assert_eq!(blocked_refused[0]["code"], json!("agent_not_found"), "{blocked_refused:?}");

    // The prior pending request from A is now denied -- B can no longer
    // accept it (proof, without a direct SQL probe, that it's no longer
    // `pending`); A gets no notification of any kind (no error-shaped
    // signal exists for it -- A simply keeps getting agent_not_found).
    let accept_err = client_b
        .tools_call("host.agent.contact_accept", json!({"request_id": request_id}))
        .await
        .expect_err("no longer pending");
    assert_eq!(accept_err.error_code.as_deref(), Some("contact_request_not_found"), "{accept_err:?}");
}

#[tokio::test]
async fn ac8_closed_policy_refuses_an_urgent_send() {
    let server = TestServer::start().await;
    let (_ns_a, key_a) = signup(&server.base_url, "Agent A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Agent B").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);

    client_b
        .tools_call("host.agent.profile_set", json!({"contact_policy": "closed"}))
        .await
        .expect("B closes contact policy");

    let raw = client_a
        .tools_call(
            "host.msg.send",
            json!({"to": [ns_b.clone()], "body": "ping", "urgent": true}),
        )
        .await
        .expect("call itself succeeds, recipient is refused");
    let result = extract_structured(&raw);
    assert_eq!(result["delivered_to"], json!([]), "{result:?}");
    let refused = result["refused"].as_array().expect("refused array");
    assert_eq!(refused[0]["address"], json!(ns_b), "{result:?}");
    assert_eq!(refused[0]["code"], json!("contact_refused"), "{result:?}");

    let inbox_raw = client_b.tools_call("host.msg.inbox", json!({})).await.expect("B inbox");
    let inbox = extract_structured(&inbox_raw);
    assert_eq!(inbox["messages"].as_array().unwrap().len(), 0, "{inbox:?}");
}

#[tokio::test]
async fn ac9_deleting_the_requester_removes_its_pending_request() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "Agent A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Agent B").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    client_b
        .tools_call("host.agent.profile_set", json!({"contact_policy": "contacts"}))
        .await
        .expect("B sets contacts policy");

    client_a
        .tools_call("host.agent.contact_request", json!({"address": ns_b.clone()}))
        .await
        .expect("A requests contact with B");

    let before = extract_structured(
        &client_b.tools_call("host.agent.contacts", json!({})).await.expect("B contacts before delete"),
    );
    assert_eq!(before["incoming"].as_array().unwrap().len(), 1, "{before:?}");

    admin
        .tools_call("admin.tenant_delete", json!({"tenant": ns_a.clone()}))
        .await
        .expect("admin.tenant_delete");

    let after = extract_structured(
        &client_b.tools_call("host.agent.contacts", json!({})).await.expect("B contacts after delete"),
    );
    let incoming = after["incoming"].as_array().expect("incoming array");
    assert!(incoming.iter().all(|r| r["address"] != json!(ns_a)), "{after:?}");
    assert_eq!(incoming.len(), 0, "{after:?}");
    assert_eq!(after["contacts"].as_array().unwrap().len(), 0, "{after:?}");
}

#[tokio::test]
async fn ac10_contact_request_surfaces_as_a_host_system_message_in_the_inbox() {
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

    let inbox_raw = client_b.tools_call("host.msg.inbox", json!({})).await.expect("B inbox");
    let inbox = extract_structured(&inbox_raw);
    let messages = inbox["messages"].as_array().expect("messages array");
    let notice = messages
        .iter()
        .find(|m| m["data"]["kind"] == json!("contact_request"))
        .unwrap_or_else(|| panic!("no contact_request system message in {inbox:?}"));
    assert_eq!(notice["from_address"], json!("host"), "{notice:?}");
    assert_eq!(notice["data"]["request_id"], json!(request_id), "{notice:?}");

    let accept_raw = client_b
        .tools_call("host.agent.contact_accept", json!({"request_id": request_id}))
        .await
        .expect("B accepts using the request_id read from its inbox");
    assert_eq!(extract_structured(&accept_raw)["status"], json!("accepted"));
}
