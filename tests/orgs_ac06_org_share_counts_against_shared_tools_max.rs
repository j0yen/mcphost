//! PRD-mcphost-org-shared-tools-namespace-v2
//! AC6 — Given O's `shared_tools_max` reached and a private tool, When O
//! shares it as `org`, Then the shipped `share_quota_exceeded` with `limit`
//! and `used` (`sharing.rs:394-396`) and nothing is shared.

use crate::common;
use common::{McpClient, TestServer, publish, signup};
use serde_json::json;

#[tokio::test]
async fn orgs_ac06() {
    let server = TestServer::start().await;
    let cap = server.state.plans.get("free").expect("free plan").shared_tools_max;
    let (_ns, key) = signup(&server.base_url, "Org Owner").await;
    let owner = McpClient::with_bearer(&server.base_url, &key);
    owner.tools_call("host.org.create", json!({"name": "acme"})).await.expect("create");

    // Fill the cap with org shares, then keep one tool private.
    for i in 0..cap {
        let name = format!("tool{i}");
        publish(&owner, &name, "echo", json!({"schema": {"type": "object"}})).await;
        owner.tools_call("host.tool_share", json!({"name": name, "visibility": "org"})).await.expect("share within the cap");
    }
    publish(&owner, "extra", "echo", json!({"schema": {"type": "object"}})).await;

    let err = owner
        .tools_call("host.tool_share", json!({"name": "extra", "visibility": "org"}))
        .await
        .expect_err("a share past shared_tools_max is refused");
    assert_eq!(err.error_code.as_deref(), Some("share_quota_exceeded"), "{err:?}");
    // The shipped error carries `limit` and `used` in its message.
    assert_eq!(err.message, format!("tenant already shares {cap} tools, the plan maximum of {cap}"), "{err:?}");

    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let visibility: String =
        conn.query_row("SELECT visibility FROM tools WHERE name = 'extra'", [], |r| r.get(0)).expect("tool row");
    assert_eq!(visibility, "private", "nothing was shared");
    let org_tools: i64 =
        conn.query_row("SELECT COUNT(*) FROM tools WHERE visibility = 'org'", [], |r| r.get(0)).expect("count");
    assert_eq!(org_tools, i64::from(cap as i32), "only the in-cap tools are org-shared");
}
