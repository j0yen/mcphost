//! PRD-mcphost-tool-call-host-verb-forward
//! AC4 — Given `host.tool_call name="admin.tenants_list"`, When it runs,
//! Then the response is `tool_not_found` with `data.host_verb:
//! "admin.tenants_list"`, `data.call_instead.tool: "admin_tenants_list"`,
//! and no admin verb executed.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn admin_dotted_name_is_refused_with_call_instead_and_never_forwarded() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call("host.tool_call", json!({"name": "admin.tenants_list", "args": {}}))
        .await
        .expect_err("an admin.* name must never be forwarded or dispatched");

    assert_eq!(err.error_code.as_deref(), Some("tool_not_found"), "{err:?}");
    assert_eq!(err.data["host_verb"], "admin.tenants_list", "{:?}", err.data);
    assert_eq!(err.data["call_instead"]["tool"], "admin_tenants_list", "{:?}", err.data);

    // No admin verb executed: this tenant's own account is untouched (not
    // disabled, not deleted) -- a later call on the same key still
    // succeeds exactly as if the admin.* call had never been sent.
    let whoami = extract_structured(
        &client
            .tools_call("host.whoami", json!({}))
            .await
            .expect("the tenant's own key must still work after the refused admin.* call"),
    );
    assert!(whoami.get("namespace").is_some(), "{whoami}");
}
