//! PRD-mcphost-time-to-first-success-public AC6 (P0) — Given admin auth, When
//! `host.whoami` runs, Then no `activation` field is present; given a tenant
//! created before the `origin` migration (`origin = 'unclassified'`), Then it
//! is excluded from `onboarding_7d`.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, ttfs_tenant};
use serde_json::json;

#[tokio::test]
async fn admin_whoami_omits_activation_and_pre_origin_rows_are_not_external() {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let whoami = extract_structured(&admin.tools_call("host.whoami", json!({})).await.expect("admin whoami"));
    assert_eq!(whoami["admin"], json!(true), "{whoami}");
    assert!(whoami.get("activation").is_none(), "admin whoami must not carry activation: {whoami}");

    let db = &server.state.db;
    let now = mcphost::state::now_unix();
    ttfs_tenant(db, "ttfs6-external", "external", None, now - 600, [Some(1), Some(5), Some(9), None]).await;
    ttfs_tenant(db, "ttfs6-legacy", "unclassified", None, now - 600, [Some(1), Some(5), Some(9), None]).await;
    let block = mcphost::funnel::onboarding_7d(db, now, 1).await.expect("compute");
    assert_eq!(block["n"], json!(1), "the pre-migration row must not count as external: {block}");
}
