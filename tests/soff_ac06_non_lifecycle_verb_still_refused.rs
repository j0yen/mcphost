//! AC6 — a non-lifecycle verb (`host.whoami`) on a disabled tenant by bearer
//! header still returns `tenant_disabled`.  This pins the no-widening
//! property: `lets_through_disabled` is narrow, not a blanket exemption.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn soff_ac06_non_lifecycle_verb_still_refused() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Non Lifecycle Verb").await;

    // Offboard via arg (the path that has always worked) to disable the tenant.
    McpClient::new(&server.base_url)
        .tools_call("host.self.offboard", json!({"tenant_key": key}))
        .await
        .expect("offboard to disable the tenant");

    // A non-lifecycle verb via bearer must still be refused.
    let err = McpClient::with_bearer(&server.base_url, &key)
        .tools_call("host.whoami", json!({}))
        .await
        .expect_err("host.whoami on a disabled tenant must be refused, not let through");
    assert_eq!(
        err.error_code.as_deref(),
        Some("tenant_disabled"),
        "non-lifecycle verb after offboard: {err:?}"
    );
}
