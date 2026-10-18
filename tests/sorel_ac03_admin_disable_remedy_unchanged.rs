//! PRD-mcphost-self-offboard-releases-session
//! AC3 (P0) — Given an admin-disabled tenant, When its key calls any
//! `host.*`, Then the error is `tenant_disabled` with `data.disabled_reason`
//! `admin_disable`, `retryable` false, and the remedy's `admin_disable`
//! branch reads "do not retry".

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn sorel_ac03_admin_disabled_key_is_refused_do_not_retry() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Sorel Admin Disable").await;
    McpClient::with_bearer(&server.base_url, ADMIN_KEY)
        .tools_call("admin.tenant_disable", json!({"tenant": ns}))
        .await
        .expect("admin.tenant_disable");

    let client = McpClient::with_bearer(&server.base_url, &key);
    for verb in ["host.whoami", "host.tool.list", "host.self.offboard"] {
        let err = client.tools_call(verb, json!({})).await.expect_err("an admin-disabled key must be refused");
        assert_eq!(err.error_code.as_deref(), Some("tenant_disabled"), "{verb}: {err:?}");
        assert_eq!(err.data["disabled_reason"], json!("admin_disable"), "{verb}: {err:?}");
        // `retryable` lives on the registry row, not in the wire `data`.
        let row = mcphost::error_registry::lookup("tenant_disabled").expect("tenant_disabled row");
        assert!(!row.retryable, "{verb}: tenant_disabled must stay non-retryable");
        let remedy = err.data["remedy"].as_str().expect("remedy string");
        let branch = remedy.split("admin_disable:").nth(1).unwrap_or_default();
        assert!(branch.contains("do not retry"), "{verb}: admin_disable branch must read do not retry: {remedy}");
    }
}
