//! PRD-mcphost-org-cost-attribution
//! AC4 — Given an org whose owner is not verified, When the weekly job runs,
//! Then no email is sent and the log line is `skipped:no_verified_owner`;
//! Given a verified owner with `digest_enabled = 0`, Then `skipped:disabled`.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::db::OperatorProof;
use mcphost::email::FakeEmailClient;
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn orgc_ac04() {
    let fake = Arc::new(FakeEmailClient::new());
    let server = TestServer::start_with_email(fake.clone()).await;
    let mut ids = Vec::new();
    for (who, org) in [("Unverified", "org-unverified"), ("Verified", "org-disabled")] {
        let (ns, key) = signup(&server.base_url, who).await;
        let c = McpClient::with_bearer(&server.base_url, &key);
        c.tools_call("host.org.create", json!({"name": org})).await.expect("create");
        ids.push(server.state.db.find_tenant_by_namespace(ns).await.expect("lookup").expect("tenant").id);
    }
    server
        .state
        .db
        .verify_owner_as(
            ids[1],
            "email".to_string(),
            "owner@example.com".to_string(),
            Some(OperatorProof { email: "owner@example.com".to_string(), actor: "test".to_string() }),
        )
        .await
        .expect("verify");
    assert!(server.state.db.set_org_digest_enabled(ids[1], false).await.expect("disable"));

    let lines = mcphost::org_digest::weekly_tick(&server.state, mcphost::state::now_unix()).await.expect("tick");
    assert!(lines.contains(&"org=org-unverified skipped:no_verified_owner".to_string()), "{lines:?}");
    assert!(lines.contains(&"org=org-disabled skipped:disabled".to_string()), "{lines:?}");
    assert_eq!(fake.send_count(), 0, "a skipped org must send nothing");
    assert!(lines.iter().all(|l| !l.contains('@')), "no address in the log: {lines:?}");
}
