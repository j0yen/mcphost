//! PRD-mcphost-oauth-discovery-document-contract
//! AC7 (P0) — Given two concurrent `issuer_remove` calls for the same
//! issuer, When both complete, Then exactly one returns `{removed: true}`,
//! the other `not_found`, and no panic or duplicate audit row occurs.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn oauthdoc_ac07_concurrent_issuer_remove_has_one_winner() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Race Tenant").await;
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.unwrap().expect("tenant row");
    McpClient::with_bearer(&server.base_url, &key)
        .tools_call(
            "host.oauth.issuer_set",
            json!({"issuer": "https://race.example.com", "audience": "aud", "jwks_url": "https://idp.example.com/jwks"}),
        )
        .await
        .expect("register issuer");

    let call = || {
        let client = McpClient::with_bearer(&server.base_url, &key);
        async move { client.tools_call("host.oauth.issuer_remove", json!({"issuer": "https://race.example.com"})).await }
    };
    let (a, b) = tokio::join!(tokio::spawn(call()), tokio::spawn(call()));
    let results = [a.expect("first call must not panic"), b.expect("second call must not panic")];

    let winners = results.iter().filter(|r| r.as_ref().is_ok_and(|v| v["structuredContent"]["removed"] == json!(true))).count();
    let not_found = results
        .iter()
        .filter(|r| r.as_ref().err().is_some_and(|e| e.error_code.as_deref() == Some("issuer_not_found")))
        .count();
    assert_eq!((winners, not_found), (1, 1), "exactly one removed:true and one not_found: {results:?}");

    let audit = server
        .state
        .db
        .list_oauth_audit(tenant.id, None, None, Some("issuer_removed".to_string()), 10, 0)
        .await
        .expect("audit rows");
    assert_eq!(audit.len(), 1, "exactly one audit row, got {}", audit.len());
}
