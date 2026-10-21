//! PRD-mcphost-connect-start-and-callback
//! AC3 (P0) -- Given `host.connect.start {provider: "testprov", scopes:
//! ["bogus"]}`, When it runs, Then the error is `scope_unknown {unknown:
//! ["bogus"]}`.

use crate::common;
use common::testoauth::TestOAuth;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn cstart_ac03() {
    let oauth = TestOAuth::start().await;
    let server = TestServer::start_with_connect_providers(oauth.connect_providers()).await;
    let (ns, key) = signup(&server.base_url, "CStart AC3").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.unwrap().expect("tenant");

    let err = client
        .tools_call("host.connect.start", json!({"provider": "testprov", "scopes": ["bogus"]}))
        .await
        .expect_err("bogus is not a testprov scope");
    assert_eq!(err.error_code.as_deref(), Some("scope_unknown"), "{err:?}");
    assert_eq!(err.data["unknown"], json!(["bogus"]), "{err:?}");

    // A known scope next to an unknown one: only the unknown one is named.
    let err = client
        .tools_call("host.connect.start", json!({"provider": "testprov", "scopes": ["testprov.read", "bogus"]}))
        .await
        .expect_err("one unknown scope refuses the call");
    assert_eq!(err.data["unknown"], json!(["bogus"]), "{err:?}");

    assert_eq!(server.state.db.count_vault_handoff_tokens(tenant.id).await.unwrap(), 0, "no link was minted");
}
