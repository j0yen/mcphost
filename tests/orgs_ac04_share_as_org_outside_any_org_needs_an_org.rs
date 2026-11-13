//! PRD-mcphost-org-shared-tools-namespace-v2
//! AC4 — Given a tenant outside any org, When it calls `host.tool_share
//! {visibility: "org"}`, Then `org_required` with `data.next_call`
//! `host.org.create`, and the tool's visibility is unchanged.

use crate::common;
use common::{McpClient, TestServer, publish, signup};
use serde_json::json;

#[tokio::test]
async fn orgs_ac04() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Loner").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    publish(&client, "tt", "echo", json!({"schema": {"type": "object"}})).await;

    let err = client
        .tools_call("host.tool_share", json!({"name": "tt", "visibility": "org"}))
        .await
        .expect_err("no org, no org share");
    assert_eq!(err.error_code.as_deref(), Some("org_required"), "{err:?}");
    assert_eq!(err.data["next_call"], json!("host.org.create"), "{:?}", err.data);

    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let visibility: String =
        conn.query_row("SELECT visibility FROM tools WHERE name = 'tt'", [], |r| r.get(0)).expect("tool row");
    assert_eq!(visibility, "private", "the refused share changed nothing");
}
