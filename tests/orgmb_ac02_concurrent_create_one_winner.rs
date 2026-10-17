//! PRD-mcphost-org-model-b
//! AC2 — Given two concurrent `host.org.create {name: "acme"}` calls from
//! different tenants, When both commit, Then exactly one succeeds and the
//! other gets `org_name_taken`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn orgmb_ac02() {
    let server = TestServer::start().await;
    let (_a_ns, a_key) = signup(&server.base_url, "Racer A").await;
    let (_b_ns, b_key) = signup(&server.base_url, "Racer B").await;
    let a = McpClient::with_bearer(&server.base_url, &a_key);
    let b = McpClient::with_bearer(&server.base_url, &b_key);

    let (ra, rb) = tokio::join!(
        a.tools_call("host.org.create", json!({"name": "acme"})),
        b.tools_call("host.org.create", json!({"name": "acme"})),
    );

    let (ok, err) = match (ra, rb) {
        (Ok(v), Err(e)) | (Err(e), Ok(v)) => (v, e),
        (Ok(_), Ok(_)) => panic!("both creates succeeded: the org name is not unique"),
        (Err(x), Err(y)) => panic!("both creates failed: {x:?} / {y:?}"),
    };
    let created = extract_structured(&ok);
    assert_eq!(created["org"], json!("acme"), "{created}");
    assert_eq!(created["role"], json!("owner"), "{created}");
    assert_eq!(err.error_code.as_deref(), Some("org_name_taken"), "{err:?}");
}
