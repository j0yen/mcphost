//! PRD-mcphost-connect-start-and-callback
//! AC2 (P0) -- Given provider `github` with `MCPHOST_CONNECT_GITHUB_CLIENT_ID`
//! unset, When `host.connect.start {provider: "github"}` runs, Then the error
//! is `provider_unavailable {provider: "github", reason:
//! "client_id_not_configured"}` and no handoff row is written.

use crate::common;
use common::testoauth::TestOAuth;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn cstart_ac02() {
    let oauth = TestOAuth::start().await;
    // Only `testprov` has credentials; `github`'s client id is not configured.
    let server = TestServer::start_with_connect_providers(oauth.connect_providers()).await;
    let (ns, key) = signup(&server.base_url, "CStart AC2").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.unwrap().expect("tenant");

    let err = client
        .tools_call("host.connect.start", json!({"provider": "github"}))
        .await
        .expect_err("github is not configured on this host");
    assert_eq!(err.error_code.as_deref(), Some("provider_unavailable"), "{err:?}");
    assert_eq!(err.data["provider"], "github", "{err:?}");
    assert_eq!(err.data["reason"], "client_id_not_configured", "{err:?}");

    assert_eq!(
        server.state.db.count_vault_handoff_tokens(tenant.id).await.unwrap(),
        0,
        "an unavailable provider writes no handoff row"
    );
}
