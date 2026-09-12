//! PRD-mcphost-tool-test AC7 — Given a request with more than five
//! invocations, When tested, Then it is refused with an error naming the
//! limit, and zero invocations run.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn six_invocations_are_refused_and_none_run() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Too Many Invocations Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({"schema": {"type": "object"}});
    let six: Vec<serde_json::Value> = (0..6).map(|_| json!({})).collect();
    let err = client
        .tools_call(
            "host.spec_test",
            json!({"kind": "echo", "spec": spec, "invocations": six}),
        )
        .await
        .expect_err("more than 5 invocations must be refused");
    assert_eq!(err.error_code.as_deref(), Some("too_many_invocations"));
    assert_eq!(err.data["limit"], json!(5));

    // Zero invocations ran: usage over the current window is still empty.
    let usage = client
        .tools_call("host.usage", json!({}))
        .await
        .expect("usage ok");
    assert_eq!(extract_structured(&usage)["calls"], json!(0));
}
