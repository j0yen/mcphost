//! PRD-mcphost-connection-provider-registry AC3 (P0) -- Given
//! `MCPHOST_CONNECT_GITHUB_CLIENT_ID` set and
//! `MCPHOST_CONNECT_GITHUB_CLIENT_SECRET` unset, When `host.connect.providers`
//! runs, Then `github` has `available: false, reason:
//! "client_secret_not_configured"` and the response contains neither env value.

use crate::common;
use common::{extract_structured, signup};
use serde_json::json;

const ID_VAR: &str = "MCPHOST_CONNECT_GITHUB_CLIENT_ID";
const SECRET_VAR: &str = "MCPHOST_CONNECT_GITHUB_CLIENT_SECRET";
const ID_VALUE: &str = "cprov-ac03-client-id-value";

#[tokio::test]
async fn cprov_ac03() {
    // `EnvGuard` serializes every guarded env span behind one process-wide
    // lock, so a test can hold exactly one guard at a time (a second `set`
    // would wait on the first forever). The client id is the variable this
    // test mutates; the secret is never set anywhere in the suite, so it is
    // asserted absent rather than cleared.
    let _id = common::EnvGuard::set(ID_VAR, ID_VALUE).await;
    assert!(std::env::var(SECRET_VAR).is_err(), "{SECRET_VAR} must be unset for this test");
    let server = common::TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "CPROV3 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let result = client.tools_call("host.connect.providers", json!({})).await.expect("providers");
    let raw = result.to_string();
    let listed = extract_structured(&result);
    let github = listed["providers"]
        .as_array()
        .expect("providers")
        .iter()
        .find(|p| p["name"] == "github")
        .expect("github row");
    assert_eq!(github["available"], json!(false), "{github}");
    assert_eq!(github["reason"], json!("client_secret_not_configured"), "{github}");
    assert!(!raw.contains(ID_VALUE), "the client id must not appear in the response: {raw}");
}
