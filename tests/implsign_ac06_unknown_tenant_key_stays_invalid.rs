//! PRD-mcphost-implicit-signup
//! AC6 (P0) — Given a request with an unknown `tenant_key`, When
//! dispatched, Then `tenant_key_invalid` is returned and no tenant is
//! created.
//!
//! Requirement 4: `tenant_key_missing` is no longer reachable for
//! `host.*`/`billing.*` on `/mcp` (see implsign_ac01/ac05), but
//! `tenant_key_invalid` is unchanged -- an *explicit*, unrecognized key
//! must never be quietly rescued by implicit signup (the new
//! `Auth::Anonymous` arm only ever matches when `auth` is still
//! `Anonymous`; an unrecognized `tenant_key` resolves to `Auth::Invalid`
//! instead, which that arm's pattern excludes).

use crate::common;
use common::{McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn unknown_tenant_key_argument_is_still_invalid_not_implicitly_rescued() {
    let server = TestServer::start().await;
    let before = server.state.db.list_tenants().await.unwrap().len();

    let client = McpClient::new(&server.base_url);
    let err = client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "hello",
                "kind": "echo",
                "spec": {"schema": {"type": "object"}},
                "tenant_key": "mph_definitely-not-a-real-key",
            }),
        )
        .await
        .expect_err("an unrecognized tenant_key must still be refused");

    assert_eq!(
        err.error_code.as_deref(),
        Some("tenant_key_invalid"),
        "{err:?}"
    );

    let after = server.state.db.list_tenants().await.unwrap().len();
    assert_eq!(
        after, before,
        "an unrecognized tenant_key must never create a tenant"
    );
}
