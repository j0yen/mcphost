//! PRD-mcphost-oauth-discovery-document-contract
//! AC8 (P1) — Given an empty `oauth_issuers` table, When the root document
//! is built, Then it lists exactly the hosted authorization server and the
//! doctor reports `{rows: 0, invalid: []}`.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, signup};
use serde_json::{Value, json};

#[tokio::test]
async fn oauthdoc_ac08_empty_table_lists_only_hosted_as() {
    let server = TestServer::start().await;
    let (ns, _key) = signup(&server.base_url, "Empty Table Tenant").await;

    let root: Value = reqwest::get(format!("{}/.well-known/oauth-protected-resource", server.base_url))
        .await
        .expect("GET root document")
        .json()
        .await
        .expect("parse JSON");
    assert_eq!(root["authorization_servers"], json!([server.base_url]), "only the hosted AS: {root}");

    let doctor = McpClient::with_bearer(&server.base_url, ADMIN_KEY)
        .tools_call("host.doctor", json!({"tenant": ns}))
        .await
        .expect("admin host.doctor");
    assert_eq!(doctor["structuredContent"]["oauth_issuers"], json!({"rows": 0, "invalid": []}));
}
