//! PRD-mcphost-second-session-nudge
//! AC3 (P0) — Given an unclaimed tenant 30 h after its first call, When the
//! sweep runs, Then no send happens and the row has `nudge_channel = none`
//! with `nudged_unix` set.

use crate::common;
use common::{McpClient, TestServer};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn unclaimed_tenant_is_recorded_as_none_with_no_send() {
    let fake = Arc::new(mcphost::email::FakeEmailClient::new());
    let server = TestServer::start_with_email(fake.clone()).await;
    let client = McpClient::new(&server.base_url);

    let raw = client
        .tools_call("signup", json!({"name": "AC3 Tenant"}))
        .await
        .expect("signup");
    let signup = common::extract_structured(&raw);
    let namespace = signup["tenant"].as_str().expect("tenant").to_string();
    let key = signup["key"].as_str().expect("key").to_string();

    let tenant_client = McpClient::with_bearer(&server.base_url, &key);
    tenant_client.tools_call("host.whoami", json!({})).await.expect("whoami");

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(namespace)
        .await
        .expect("find tenant")
        .expect("tenant exists");
    assert!(tenant.owner_verified_at.is_none(), "tenant must be unclaimed");

    let backdated = mcphost::state::now_unix() - 30 * 3600;
    server
        .state
        .db
        .set_tenant_stamp_for_test(tenant.id, "first_call_unix", backdated)
        .await
        .expect("backdate first_call_unix");

    let report = mcphost::returns::sweep(&server.state).await.expect("sweep");
    assert_eq!(report.selected, 1, "{report:?}");
    assert_eq!(report.none, 1, "{report:?}");
    assert_eq!(report.emailed, 0, "{report:?}");
    assert_eq!(fake.send_count(), 0, "an unclaimed tenant must never be emailed");

    let (nudged_unix, nudge_channel, _attempts, _second_session_unix) = server
        .state
        .db
        .nudge_status(tenant.id)
        .await
        .expect("nudge_status")
        .expect("tenant exists");
    assert!(nudged_unix.is_some(), "nudged_unix must be set");
    assert_eq!(nudge_channel.as_deref(), Some("none"));
}
