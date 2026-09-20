//! PRD-mcphost-agent-mesh-ops
//! AC7 (P0) -- Given tenant Z that has sent, received, acked, blocked,
//! requested, accepted, opened a channel, posted and stored a cursor,
//! When `admin.tenant_delete(Z)` runs, Then no row in
//! `thread_participants`, `message_receipts`, `blocks`, `contacts`,
//! `contact_requests`, `channel_cursors`, `agent_profiles` or
//! message-kind `triggers` references Z's id, and Z's sent messages have
//! `from_tenant_id` null with `from_address` intact.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use rusqlite::params;
use serde_json::json;

fn count_refs(db_path: &std::path::Path, sql: &str, tenant_id: i64) -> i64 {
    let conn = rusqlite::Connection::open(db_path).expect("open raw db");
    conn.query_row(sql, params![tenant_id], |r| r.get(0))
        .expect("count query")
}

#[tokio::test]
async fn ac7_tenant_delete_leaves_no_row_referencing_z_across_messaging_tables() {
    let server = TestServer::start_with_signup_rate_limit(100).await;
    let db_path = server.data_dir.0.join("mcphost.db");

    let (ns_z, key_z) = signup(&server.base_url, "Tenant Z").await;
    let (ns_y, _key_y) = signup(&server.base_url, "Tenant Y").await;
    let (_ns_w, key_w) = signup(&server.base_url, "Tenant W").await;
    let (ns_v, _key_v) = signup(&server.base_url, "Tenant V").await;
    let (ns_c, key_c) = signup(&server.base_url, "Tenant C").await;
    let (_ns_u, key_u) = signup(&server.base_url, "Tenant U").await;
    let client_z = McpClient::with_bearer(&server.base_url, &key_z);
    let client_w = McpClient::with_bearer(&server.base_url, &key_w);
    let client_c = McpClient::with_bearer(&server.base_url, &key_c);
    let client_u = McpClient::with_bearer(&server.base_url, &key_u);
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    // Sent.
    let sent_raw = client_z
        .tools_call("host.msg.send", json!({"to": [ns_y], "body": "z to y"}))
        .await
        .expect("Z sends to Y");
    let sent_message_id = extract_structured(&sent_raw)["message_id"]
        .as_str()
        .expect("message_id")
        .to_string();

    // Received + acked.
    let recv_raw = client_w
        .tools_call("host.msg.send", json!({"to": [ns_z.clone()], "body": "w to z"}))
        .await
        .expect("W sends to Z");
    let recv_message_id = extract_structured(&recv_raw)["message_id"]
        .as_str()
        .expect("message_id")
        .to_string();
    client_z
        .tools_call("host.msg.ack", json!({"message_ids": [recv_message_id]}))
        .await
        .expect("Z acks");

    // Blocked.
    client_z
        .tools_call("host.msg.block", json!({"address": ns_v}))
        .await
        .expect("Z blocks V");

    // Requested: Z requests contact with C (contacts policy).
    client_c
        .tools_call("host.agent.profile_set", json!({"contact_policy": "contacts"}))
        .await
        .expect("C sets contacts policy");
    client_z
        .tools_call("host.agent.contact_request", json!({"address": ns_c}))
        .await
        .expect("Z requests contact with C");

    // Accepted: U requests contact with Z (Z is contacts-policy, which
    // also gives Z an `agent_profiles` row), Z accepts.
    client_z
        .tools_call("host.agent.profile_set", json!({"contact_policy": "contacts"}))
        .await
        .expect("Z sets contacts policy");
    let req_raw = client_u
        .tools_call("host.agent.contact_request", json!({"address": ns_z.clone()}))
        .await
        .expect("U requests contact with Z");
    let request_id = extract_structured(&req_raw)["request_id"]
        .as_str()
        .expect("request_id")
        .to_string();
    client_z
        .tools_call("host.agent.contact_accept", json!({"request_id": request_id}))
        .await
        .expect("Z accepts U's request");

    // Opened a channel, posted, stored a cursor.
    client_z
        .tools_call("host.channel.open", json!({"name": "z-channel"}))
        .await
        .expect("Z opens a channel");
    client_z
        .tools_call("host.channel.post", json!({"channel": "z-channel", "body": "hi"}))
        .await
        .expect("Z posts");

    // A message-kind trigger, so `triggers(kind='message')` has a row for Z.
    client_z
        .tools_call(
            "host.tool_publish",
            json!({"name": "echo_tool", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("Z publishes echo_tool");
    client_z
        .tools_call("host.trigger.set", json!({"tool": "echo_tool", "kind": "message"}))
        .await
        .expect("Z sets a message trigger");

    let tenant_z = server
        .state
        .db
        .find_tenant_by_namespace(ns_z.clone())
        .await
        .expect("find Z")
        .expect("Z exists");
    let z_id = tenant_z.id;

    admin
        .tools_call("admin.tenant_delete", json!({"tenant": ns_z.clone()}))
        .await
        .expect("admin.tenant_delete");

    assert_eq!(
        count_refs(&db_path, "SELECT COUNT(*) FROM thread_participants WHERE tenant_id = ?1", z_id),
        0
    );
    assert_eq!(
        count_refs(&db_path, "SELECT COUNT(*) FROM message_receipts WHERE tenant_id = ?1", z_id),
        0
    );
    assert_eq!(
        count_refs(
            &db_path,
            "SELECT COUNT(*) FROM blocks WHERE tenant_id = ?1 OR blocked_tenant_id = ?1",
            z_id
        ),
        0
    );
    assert_eq!(
        count_refs(
            &db_path,
            "SELECT COUNT(*) FROM contacts WHERE tenant_id = ?1 OR contact_tenant_id = ?1",
            z_id
        ),
        0
    );
    assert_eq!(
        count_refs(
            &db_path,
            "SELECT COUNT(*) FROM contact_requests WHERE from_tenant_id = ?1 OR to_tenant_id = ?1",
            z_id
        ),
        0
    );
    assert_eq!(
        count_refs(&db_path, "SELECT COUNT(*) FROM channel_cursors WHERE tenant_id = ?1", z_id),
        0
    );
    assert_eq!(
        count_refs(&db_path, "SELECT COUNT(*) FROM agent_profiles WHERE tenant_id = ?1", z_id),
        0
    );
    assert_eq!(
        count_refs(
            &db_path,
            "SELECT COUNT(*) FROM triggers WHERE tenant_id = ?1 AND kind = 'message'",
            z_id
        ),
        0
    );

    // Z's sent message survives with from_tenant_id NULL, from_address intact.
    let conn = rusqlite::Connection::open(&db_path).expect("open raw db");
    let (from_tenant_id, from_address): (Option<i64>, String) = conn
        .query_row(
            "SELECT from_tenant_id, from_address FROM messages WHERE id = ?1",
            params![sent_message_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("query sent message");
    assert_eq!(from_tenant_id, None, "from_tenant_id must be null after delete");
    assert_eq!(from_address, ns_z, "from_address must remain intact");
}
