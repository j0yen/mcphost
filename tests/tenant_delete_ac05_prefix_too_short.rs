//! PRD-mcphost-tenant-delete
//! AC5 — Given a prefix of fewer than 4 characters or an empty prefix,
//! When either batch call runs, Then it returns `invalid_params` and
//! nothing changes.

mod common;
use common::{ADMIN_KEY, McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn short_or_empty_prefix_is_refused_for_both_dry_run_and_real_delete() {
    let server = TestServer::start().await;
    let (ns, _) = signup(&server.base_url, "panel_short_guard").await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    for prefix in ["", "a", "ab", "abc"] {
        let err = admin
            .tools_call("admin.tenant_delete_by_prefix", json!({"prefix": prefix}))
            .await
            .expect_err(&format!("prefix {prefix:?} (dry run) must be refused"));
        assert_eq!(err.error_code.as_deref(), Some("invalid_params"));

        let err = admin
            .tools_call(
                "admin.tenant_delete_by_prefix",
                json!({"prefix": prefix, "dry_run": false}),
            )
            .await
            .expect_err(&format!("prefix {prefix:?} (dry_run=false) must be refused"));
        assert_eq!(err.error_code.as_deref(), Some("invalid_params"));
    }

    // A typo-length prefix must not touch anything, including a tenant
    // that would otherwise match a longer, correct prefix.
    assert!(
        server
            .state
            .db
            .find_tenant_by_namespace(ns)
            .await
            .unwrap()
            .is_some()
    );
}
