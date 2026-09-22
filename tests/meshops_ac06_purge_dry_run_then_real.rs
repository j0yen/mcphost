//! PRD-mcphost-agent-mesh-ops
//! AC6 (P0) -- Given messages aged 40 and 10 days, When the admin calls
//! `admin.mesh.purge(older_than_days=30, dry_run=true)`, Then the response
//! reports the 40-day count and nothing is deleted; and with
//! `dry_run=false`, Then those rows and their receipts are gone, the
//! 10-day rows remain, and channel cursors below the new horizon read
//! from the first retained `seq`.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use rusqlite::{OptionalExtension, params};
use serde_json::json;

fn message_exists(db_path: &std::path::Path, message_id: &str) -> bool {
    let conn = rusqlite::Connection::open(db_path).expect("open raw db");
    conn.query_row(
        "SELECT 1 FROM messages WHERE id = ?1",
        params![message_id],
        |r| r.get::<_, i64>(0),
    )
    .optional()
    .expect("query messages")
    .is_some()
}

fn receipt_count(db_path: &std::path::Path, message_id: &str) -> i64 {
    let conn = rusqlite::Connection::open(db_path).expect("open raw db");
    conn.query_row(
        "SELECT COUNT(*) FROM message_receipts WHERE message_id = ?1",
        params![message_id],
        |r| r.get(0),
    )
    .expect("count receipts")
}

fn channel_post_exists(db_path: &std::path::Path, post_id: &str) -> bool {
    let conn = rusqlite::Connection::open(db_path).expect("open raw db");
    conn.query_row(
        "SELECT 1 FROM channel_posts WHERE id = ?1",
        params![post_id],
        |r| r.get::<_, i64>(0),
    )
    .optional()
    .expect("query channel_posts")
    .is_some()
}

fn cursor_seq(db_path: &std::path::Path, channel_id: &str, tenant_id: i64) -> i64 {
    let conn = rusqlite::Connection::open(db_path).expect("open raw db");
    conn.query_row(
        "SELECT seq FROM channel_cursors WHERE channel_id = ?1 AND tenant_id = ?2",
        params![channel_id, tenant_id],
        |r| r.get(0),
    )
    .expect("query channel_cursors")
}

#[tokio::test]
async fn ac6_purge_dry_run_reports_then_real_run_deletes_and_clamps_cursors() {
    let server = TestServer::start().await;
    let db_path = server.data_dir.0.join("mcphost.db");
    let (_ns_a, key_a) = signup(&server.base_url, "Tenant A").await;
    let (ns_b, _key_b) = signup(&server.base_url, "Tenant B").await;
    let (ns_x, key_x) = signup(&server.base_url, "Tenant X").await;
    let (_ns_y, key_y) = signup(&server.base_url, "Tenant Y").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let client_x = McpClient::with_bearer(&server.base_url, &key_x);
    let client_y = McpClient::with_bearer(&server.base_url, &key_y);
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    let now_ms = mcphost::state::now_unix_ms();
    let forty_days_ms = 40 * 24 * 3_600_000_i64;
    let ten_days_ms = 10 * 24 * 3_600_000_i64;

    // A 40-day-old message and a 10-day-old one.
    let old_msg_raw = client_a
        .tools_call("host.msg.send", json!({"to": [ns_b.clone()], "body": "old"}))
        .await
        .expect("A sends old message");
    let old_message_id = extract_structured(&old_msg_raw)["message_id"]
        .as_str()
        .expect("message_id")
        .to_string();
    server
        .state
        .db
        .test_backdate_message(old_message_id.clone(), now_ms - forty_days_ms)
        .await
        .expect("backdate old message");

    let recent_msg_raw = client_a
        .tools_call("host.msg.send", json!({"to": [ns_b], "body": "recent"}))
        .await
        .expect("A sends recent message");
    let recent_message_id = extract_structured(&recent_msg_raw)["message_id"]
        .as_str()
        .expect("message_id")
        .to_string();
    server
        .state
        .db
        .test_backdate_message(recent_message_id.clone(), now_ms - ten_days_ms)
        .await
        .expect("backdate recent message");

    // A channel with one 40-day-old post (X's own, so X's cursor lands on
    // it) and one recent post from a different tenant (Y), so the channel
    // still has a surviving post after the purge.
    let open_raw = client_x
        .tools_call("host.channel.open", json!({"name": "c1"}))
        .await
        .expect("X opens channel");
    let channel_id = extract_structured(&open_raw)["channel_id"]
        .as_str()
        .expect("channel_id")
        .to_string();
    let old_post_raw = client_x
        .tools_call("host.channel.post", json!({"channel": "c1", "body": "old post"}))
        .await
        .expect("X posts old");
    let old_post_id = extract_structured(&old_post_raw)["post_id"]
        .as_str()
        .expect("post_id")
        .to_string();
    server
        .state
        .db
        .test_backdate_channel_post(old_post_id.clone(), now_ms - forty_days_ms)
        .await
        .expect("backdate old post");
    let tenant_x = server
        .state
        .db
        .find_tenant_by_namespace(ns_x)
        .await
        .expect("find X")
        .expect("X exists");
    assert_eq!(cursor_seq(&db_path, &channel_id, tenant_x.id), 1, "X's cursor starts on its own post");

    client_y
        .tools_call("host.channel.post", json!({"channel": "c1", "body": "recent post"}))
        .await
        .expect("Y posts recent");

    // Dry run: reports the 40-day count, deletes nothing.
    let dry_raw = admin
        .tools_call("admin.mesh.purge", json!({"older_than_days": 30, "dry_run": true}))
        .await
        .expect("admin.mesh.purge dry run");
    let dry = extract_structured(&dry_raw);
    assert_eq!(dry["dry_run"], json!(true), "{dry:?}");
    assert_eq!(dry["messages_removed"], json!(1), "{dry:?}");
    assert_eq!(dry["channel_posts_removed"], json!(1), "{dry:?}");
    assert!(message_exists(&db_path, &old_message_id), "dry run must not delete");
    assert!(channel_post_exists(&db_path, &old_post_id), "dry run must not delete");

    // Real run.
    let real_raw = admin
        .tools_call("admin.mesh.purge", json!({"older_than_days": 30, "dry_run": false}))
        .await
        .expect("admin.mesh.purge real run");
    let real = extract_structured(&real_raw);
    assert_eq!(real["dry_run"], json!(false), "{real:?}");
    assert_eq!(real["messages_removed"], json!(1), "{real:?}");
    assert_eq!(real["channel_posts_removed"], json!(1), "{real:?}");

    assert!(!message_exists(&db_path, &old_message_id), "old message must be gone");
    assert_eq!(receipt_count(&db_path, &old_message_id), 0, "its receipts must be gone");
    assert!(message_exists(&db_path, &recent_message_id), "10-day message must remain");

    assert!(!channel_post_exists(&db_path, &old_post_id), "old post must be gone");
    assert_eq!(
        cursor_seq(&db_path, &channel_id, tenant_x.id),
        2,
        "X's cursor must clamp up to the first retained seq"
    );
}
