//! PRD-mcphost-agent-mesh-ops
//! AC3 (P0) -- Given the same thread, When the admin calls
//! `admin.mesh.thread(thread_id)`, Then bodies are returned and exactly
//! one `admin_events` row with `action="mesh.thread_read"` and that
//! thread id exists afterwards.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use rusqlite::params;
use serde_json::json;

fn mesh_thread_read_rows_for(db_path: &std::path::Path, thread_id: &str) -> i64 {
    let conn = rusqlite::Connection::open(db_path).expect("open raw db");
    conn.query_row(
        "SELECT COUNT(*) FROM admin_events WHERE action = 'mesh.thread_read' AND tenant = ?1",
        params![thread_id],
        |r| r.get(0),
    )
    .expect("count admin_events")
}

#[tokio::test]
async fn ac3_thread_returns_bodies_and_writes_one_audit_row() {
    let server = TestServer::start().await;
    let db_path = server.data_dir.0.join("mcphost.db");
    let (ns_a, key_a) = signup(&server.base_url, "Agent A").await;
    let (ns_b, _key_b) = signup(&server.base_url, "Agent B").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);

    let send_raw = client_a
        .tools_call("host.msg.send", json!({"to": [ns_b], "body": "the actual body"}))
        .await
        .expect("A sends to B");
    let thread_id = extract_structured(&send_raw)["thread_id"]
        .as_str()
        .expect("thread_id")
        .to_string();

    assert_eq!(mesh_thread_read_rows_for(&db_path, &thread_id), 0, "no read yet");

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let raw = admin
        .tools_call("admin.mesh.thread", json!({"thread_or_channel_id": thread_id}))
        .await
        .expect("admin.mesh.thread");
    let result = extract_structured(&raw);

    let messages = result["messages"].as_array().expect("messages array");
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert_eq!(messages[0]["body"], json!("the actual body"), "{messages:?}");
    assert_eq!(messages[0]["from_address"], json!(ns_a), "{messages:?}");

    assert_eq!(
        mesh_thread_read_rows_for(&db_path, &thread_id),
        1,
        "exactly one mesh.thread_read row for this thread id"
    );
}
