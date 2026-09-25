//! PRD-mcphost-abuse-guard-ban-list
//! AC6 (P0) — Given a ban whose `expires_at` is 2 s away, When 3 s pass
//! and the subject calls, Then the call succeeds without a restart.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn a_ban_stops_enforcing_at_its_own_expiry_with_no_restart() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Briefly Banned Tenant").await;
    let tenant = server
        .state
        .db
        .find_tenant_by_key_hash(mcphost::auth::hash_key(&key))
        .await
        .expect("find tenant")
        .expect("tenant exists");
    let client = McpClient::with_bearer(&server.base_url, &key);

    let now = mcphost::state::now_unix();
    server
        .state
        .db
        .insert_ban(
            "key".to_string(),
            tenant.key_hash.clone(),
            "test".to_string(),
            false,
            "test".to_string(),
            Some(now + 2),
            false,
        )
        .await
        .expect("insert_ban");
    server.state.bans.refresh(&server.state.db).await.expect("refresh");

    let err = client
        .tools_call("host.whoami", json!({}))
        .await
        .expect_err("still within the 2s window, the call must be refused");
    assert_eq!(err.error_code.as_deref(), Some("banned"));

    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    client
        .tools_call("host.whoami", json!({}))
        .await
        .unwrap_or_else(|e| panic!("past expires_at, the call must succeed with no restart: {e:?}"));
}
