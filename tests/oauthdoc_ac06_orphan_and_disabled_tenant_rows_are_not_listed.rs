//! PRD-mcphost-oauth-discovery-document-contract
//! AC6 (P0) — Given a disabled tenant and an orphan row whose tenant id no
//! longer exists, When the root document is built, Then neither row is
//! listed and the doctor reports the orphan with reason `tenant_missing`.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, signup};
use serde_json::{Value, json};

#[tokio::test]
async fn oauthdoc_ac06_orphan_and_disabled_tenant_rows_are_not_listed() {
    let server = TestServer::start().await;
    let (live_ns, live_key) = signup(&server.base_url, "Live Tenant").await;
    let (gone_ns, _gone_key) = signup(&server.base_url, "Disabled Tenant").await;
    let live = server.state.db.find_tenant_by_namespace(live_ns.clone()).await.unwrap().expect("live tenant");
    let gone = server.state.db.find_tenant_by_namespace(gone_ns.clone()).await.unwrap().expect("gone tenant");

    let orphan_tenant_id = gone.id + 1000;
    for (tenant_id, issuer) in [
        (live.id, "https://live.example.com"),
        (gone.id, "https://disabled.example.com"),
        (orphan_tenant_id, "https://orphan.example.com"),
    ] {
        server
            .state
            .db
            .insert_oauth_issuer(
                tenant_id,
                issuer.to_string(),
                "aud".to_string(),
                "https://idp.example.com/jwks".to_string(),
                "2026-10-14T00:00:00Z".to_string(),
            )
            .await
            .unwrap_or_else(|e| panic!("insert {issuer}: {e}"));
    }
    assert!(server.state.db.set_tenant_disabled(gone_ns.clone(), true).await.unwrap(), "disable tenant");
    let orphan_row = server
        .state
        .db
        .find_oauth_issuer_by_issuer("https://orphan.example.com".to_string())
        .await
        .unwrap()
        .expect("orphan row");

    let root: Value = reqwest::get(format!("{}/.well-known/oauth-protected-resource", server.base_url))
        .await
        .expect("GET root document")
        .json()
        .await
        .expect("parse JSON");
    // The root document is sorted.
    let mut expected = vec![server.base_url.clone(), "https://live.example.com".to_string()];
    expected.sort();
    assert_eq!(
        root["authorization_servers"],
        json!(expected),
        "root lists the hosted AS and the live tenant's issuer only"
    );

    let doctor = McpClient::with_bearer(&server.base_url, ADMIN_KEY)
        .tools_call("host.doctor", json!({"tenant": live_ns}))
        .await
        .expect("admin host.doctor");
    let report = &doctor["structuredContent"]["oauth_issuers"];
    assert_eq!(report["rows"], json!(3), "{report}");
    assert_eq!(
        report["invalid"],
        json!([{"id": orphan_row.id, "tenant_id": orphan_tenant_id, "reason": "tenant_missing"}]),
        "only the orphan is reported (a disabled tenant's row is valid, just unlisted)"
    );
    // A tenant key never sees another tenant's rows, orphan or not.
    let own = McpClient::with_bearer(&server.base_url, &live_key)
        .tools_call("host.doctor", json!({}))
        .await
        .expect("tenant host.doctor");
    assert_eq!(own["structuredContent"]["oauth_issuers"], json!({"rows": 1, "invalid": []}));
}
