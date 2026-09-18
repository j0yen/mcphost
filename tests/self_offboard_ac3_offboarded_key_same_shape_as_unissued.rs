//! AC3 — Given an offboarded tenant's key, When any other `host.*`/
//! `billing.*` tool is called with it, Then a typed "unknown or disabled
//! tenant" error returns, same shape as an unissued key.
//!
//! Tested via the `tenant_key` ARGUMENT auth path (`resolve_tenant_key_auth`
//! in handler.rs), which is the one path in this codebase where a disabled
//! tenant's key and a key that was never issued at all collapse to the
//! exact same error code (`tenant_key_invalid`) -- see that function's own
//! doc comment: "unlike the header path's `AppError::TenantDisabled`... a
//! disabled tenant's key sent as the `tenant_key` argument reads as
//! `tenant_key_invalid`, the same as any other unrecognized key". This is
//! the literal, code-level meaning of AC3's "same shape as an unissued
//! key" for this host.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn offboarded_tenant_key_reads_the_same_as_a_never_issued_key() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Soon Gone").await;
    // Unauthenticated transport client: no Authorization header, so every
    // call below goes through the tenant_key ARGUMENT auth path.
    let client = McpClient::new(&server.base_url);

    client
        .tools_call("host.self_offboard", json!({"tenant_key": key}))
        .await
        .expect("self_offboard via tenant_key argument must succeed once, pre-disable");

    // host.* call with the now-offboarded key.
    let err_host = client
        .tools_call("host.whoami", json!({"tenant_key": key}))
        .await
        .expect_err("host.whoami with an offboarded tenant_key must be refused");
    assert_eq!(err_host.error_code.as_deref(), Some("tenant_key_invalid"));

    // billing.* call with the same offboarded key.
    let err_billing = client
        .tools_call("billing.status", json!({"tenant_key": key}))
        .await
        .expect_err("billing.status with an offboarded tenant_key must be refused");
    assert_eq!(err_billing.error_code.as_deref(), Some("tenant_key_invalid"));

    // A key that was never issued at all, on the exact same tool: the same
    // error code, same shape, no distinguishing "exists but disabled" from
    // "never existed" (requirement 3's own guarantee).
    let err_never_issued = client
        .tools_call(
            "host.whoami",
            json!({"tenant_key": "mk_never_issued_00000000000000000000000000"}),
        )
        .await
        .expect_err("a never-issued tenant_key must also be refused");
    assert_eq!(err_never_issued.error_code.as_deref(), Some("tenant_key_invalid"));
    assert_eq!(err_host.error_code, err_never_issued.error_code);
}
