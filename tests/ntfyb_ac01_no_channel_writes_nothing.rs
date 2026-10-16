//! PRD-mcphost-notify-user-channel-b
//! AC1 -- Given a tenant with no channel, When `host.notify.send` runs, Then
//! it fails `no_channel` with `next_call: host.notify.channel_add` and no row
//! is written.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn ntfyb_ac01() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "ntfyb ac01 tenant").await;
    let agent = McpClient::with_bearer(&server.base_url, &key);

    let err = agent
        .tools_call("host.notify.send", json!({"text": "done", "level": "info"}))
        .await
        .expect_err("a send with no channel must fail");
    assert_eq!(err.error_code.as_deref(), Some("no_channel"), "{err:?}");
    assert_eq!(err.data["next_call"], json!("host.notify.channel_add"), "{err:?}");

    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("open raw db");
    for table in ["notifies", "notify_receipts"] {
        let n: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0)).expect("count");
        assert_eq!(n, 0, "{table} must hold no row after a refused send");
    }
}
