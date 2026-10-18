//! PRD-mcphost-oauth-discovery-document-contract
//! AC3 (P0) — Given the validator's rejection table, When the generated test
//! feeds each case to the write verb and to the read filter, Then both reject
//! the same set and the test fails on the first divergence, naming the case.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::oauth::ISSUER_REJECTION_CASES;
use serde_json::{Value, json};

async fn servers(url: String) -> Vec<String> {
    let body: Value = reqwest::get(url).await.expect("GET well-known").json().await.expect("parse JSON");
    body["authorization_servers"]
        .as_array()
        .expect("authorization_servers array")
        .iter()
        .map(|v| v.as_str().expect("string entry").to_string())
        .collect()
}

#[tokio::test]
async fn oauthdoc_ac03_write_and_read_reject_the_same_set() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Parity Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server.state.db.find_tenant_by_namespace(ns.clone()).await.unwrap().expect("tenant row");
    let root_url = format!("{}/.well-known/oauth-protected-resource", server.base_url);
    let tenant_url = format!("{}/.well-known/oauth-protected-resource/t/{ns}/mcp", server.base_url);

    for (case, value) in ISSUER_REJECTION_CASES {
        // Write side: the verb refuses the case and writes no row.
        let err = client
            .tools_call(
                "host.oauth.issuer_set",
                json!({"issuer": value, "audience": "aud", "jwks_url": "https://idp.example.com/jwks"}),
            )
            .await
            .err()
            .unwrap_or_else(|| panic!("case {case:?}: write verb accepted {value:?}"));
        assert_eq!(err.error_code.as_deref(), Some("issuer_url_invalid"), "case {case:?}: {}", err.message);
        assert!(
            server.state.db.find_oauth_issuer_by_issuer((*value).to_string()).await.unwrap().is_none(),
            "case {case:?}: write verb refused but left a row"
        );

        // Read side: the same value stored directly never reaches a document.
        server
            .state
            .db
            .insert_oauth_issuer(
                tenant.id,
                (*value).to_string(),
                "aud".to_string(),
                "https://idp.example.com/jwks".to_string(),
                "2026-10-14T00:00:00Z".to_string(),
            )
            .await
            .unwrap_or_else(|e| panic!("case {case:?}: seeding the stored row failed: {e}"));
        for (doc, url) in [("root", &root_url), ("tenant", &tenant_url)] {
            let listed = servers(url.clone()).await;
            assert!(
                !listed.iter().any(|i| i == value),
                "case {case:?}: read filter listed {value:?} in the {doc} document"
            );
            assert_eq!(listed, vec![server.base_url.clone()], "case {case:?}: {doc} document gained an entry");
        }
    }
}
