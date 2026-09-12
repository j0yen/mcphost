//! PRD-mcphost-tenant-state
//! AC6 — Given a free-plan tenant at 5 MiB of state, When a tool writes one
//! more byte, Then the error is `state_quota_exceeded` with `quota:
//! "state_bytes_max"` and the tool's returned result is still delivered to
//! the caller (this module's own tool-call boundary is `host.state.set`
//! itself, so "the tool's result is still delivered" reads as: the RPC
//! call still completes with a well-formed structured error response,
//! rather than the connection dying or the whole request failing
//! uninterpretably).
//!
//! The state byte quota (5 MiB) is bigger than a single request body is
//! allowed to be (AC16/`ac16_request_body_too_large`'s 1 MiB
//! `max_request_body_bytes` transport limit), so filling it takes several
//! `host.state.set` calls into distinct keys rather than one giant write --
//! exactly the shape a real tenant hits the quota through (accumulation
//! across calls, not one oversized call).

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

/// Well under the 1 MiB transport body limit.
const CHUNK_BYTES: usize = 800_000;

/// Fills `client`'s state to exactly `quota` stored bytes, in
/// `CHUNK_BYTES`-sized (or smaller, for the last one) plain-ASCII string
/// values -- each one serializes to `value_json.len() == value.len() + 2`
/// (the wrapping quotes, no escaping needed for plain letters).
async fn fill_to_quota(client: &McpClient, quota: i64) {
    let mut remaining = quota as usize;
    let mut i = 0;
    while remaining > 0 {
        let this_chunk = remaining.min(CHUNK_BYTES).saturating_sub(2).max(1);
        let value = "a".repeat(this_chunk);
        client
            .tools_call("host.state.set", json!({"key": format!("chunk{i}"), "value": value}))
            .await
            .unwrap_or_else(|e| panic!("fill chunk {i} ({this_chunk} bytes): {e:?}"));
        remaining -= this_chunk + 2;
        i += 1;
    }
}

#[tokio::test]
async fn writing_exactly_up_to_the_quota_succeeds() {
    let server = TestServer::start().await;
    let quota = server.state.plans.get("free").expect("free plan").state_bytes_max;
    let (ns, key) = signup(&server.base_url, "Filler").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    fill_to_quota(&client, quota).await;

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .unwrap()
        .expect("tenant");
    let used = server.state.db.state_bytes_used(tenant.id).await.unwrap();
    assert_eq!(used, quota, "fill_to_quota must land exactly on the quota");
}

#[tokio::test]
async fn writing_past_state_bytes_max_fails_with_state_quota_exceeded() {
    let server = TestServer::start().await;
    let quota = server.state.plans.get("free").expect("free plan").state_bytes_max;
    let (_ns, key) = signup(&server.base_url, "Filler").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    fill_to_quota(&client, quota).await;

    let err = client
        .tools_call("host.state.set", json!({"key": "over", "value": "b"}))
        .await
        .expect_err("writing past an already-full quota must fail");
    assert_eq!(err.error_code.as_deref(), Some("state_quota_exceeded"));
    assert_eq!(err.data["quota"], json!("state_bytes_max"));
    assert_eq!(err.data["limit"], json!(quota));

    // The quota check runs before the write -- the rejected key must not
    // exist at all afterward.
    let got = extract_structured(
        &client
            .tools_call("host.state.get", json!({"key": "over"}))
            .await
            .expect("get"),
    );
    assert_eq!(got["found"], json!(false));
}
