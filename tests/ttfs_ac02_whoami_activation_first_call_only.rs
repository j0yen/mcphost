//! PRD-mcphost-time-to-first-success-public AC2 (P0) — Given a tenant with
//! only `first_call_unix` set, When `host.whoami` runs, Then
//! `activation.first_call_s` is an integer, the other three are null, and
//! `next_milestone.tool == "host.tool_publish"`.

use crate::common;
use common::{TestServer, ttfs_tenant};
use serde_json::json;

#[tokio::test]
async fn whoami_reports_seconds_since_birth_and_next_milestone() {
    let server = TestServer::start().await;
    let db = &server.state.db;
    let born = mcphost::state::now_unix() - 120;
    let tenant = ttfs_tenant(db, "ttfs2-tenant", "external", None, born, [Some(2), None, None, None]).await;

    let whoami = mcphost::control::whoami(&server.state, &tenant, None, "key", None).await.expect("whoami");
    let activation = &whoami["activation"];
    assert_eq!(activation["first_call_s"], json!(2), "{whoami}");
    assert!(activation["first_call_s"].is_i64(), "{activation}");
    for null_key in ["first_publish_s", "first_own_call_s", "second_session_s"] {
        assert!(activation[null_key].is_null(), "{null_key} must be null, never 0: {activation}");
    }
    assert_eq!(activation["next_milestone"], json!({"name": "first_publish", "tool": "host.tool_publish"}));
}
