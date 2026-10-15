//! AC2 — bearer header only (no `tenant_key` argument): `host.self.offboard`
//! returns the success envelope, and a following `host.whoami` on the same
//! bearer sees `disabled_reason == "self_offboard"` in the error data.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn soff_ac02_header_only_returns_success_and_whoami_disabled() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Header Only Leaver").await;
    // No tenant_key argument — header is the only auth.
    let client = McpClient::with_bearer(&server.base_url, &key);

    let out = client
        .tools_call("host.self.offboard", json!({}))
        .await
        .expect("header-only self_offboard must return the success envelope, not tenant_is_disabled");
    let out = extract_structured(&out);
    assert_eq!(out["disabled"], json!(true), "got {out}");
    assert_eq!(out["disabled_reason"], json!("self_offboard"), "got {out}");

    // R6: a follow-up non-lifecycle verb on the now-disabled bearer carries
    // `disabled_reason` in the error data so callers can explain themselves.
    let err = client
        .tools_call("host.whoami", json!({}))
        .await
        .expect_err("host.whoami after self_offboard must be a tenant_disabled error");
    assert_eq!(
        err.error_code.as_deref(),
        Some("tenant_disabled"),
        "whoami after header-only offboard: {err:?}"
    );
    assert_eq!(
        err.data["disabled_reason"],
        json!("self_offboard"),
        "tenant_disabled must carry disabled_reason: {err:?}"
    );
}
