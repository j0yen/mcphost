//! PRD-mcphost-end-user-identity
//! AC7 (P0) — Given a call carrying no end-user identity, When
//! `host.state.get{end_user:"self"}` runs, Then it errors
//! `end_user_required`; when it instead names an explicit subject, it
//! succeeds and the result is marked `impersonated: true`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn self_without_identity_errors_explicit_subject_succeeds_impersonated() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC7 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // No OAuth bearer, no end_user_assertion -- this call carries no
    // end-user identity of its own.
    let err = client
        .tools_call("host.state.get", json!({"key": "k", "end_user": "self"}))
        .await
        .expect_err("end_user:\"self\" with no identity must fail");
    assert_eq!(err.error_code.as_deref(), Some("end_user_required"), "{err:?}");

    // An explicit subject is allowed precisely because this call carries
    // no identity of its own, and the result is marked impersonated.
    let got = extract_structured(
        &client
            .tools_call("host.state.get", json!({"key": "k", "end_user": "u5"}))
            .await
            .expect("explicit subject must succeed when the call has no identity"),
    );
    assert_eq!(got["found"], json!(false), "{got:?}");
    assert_eq!(got["impersonated"], json!(true), "{got:?}");

    // Round trip: a set under the same explicit subject, then a get,
    // proves the impersonated read actually reached u5's own scoped row.
    client
        .tools_call("host.state.set", json!({"key": "k", "value": {"who": "u5"}, "end_user": "u5"}))
        .await
        .expect("explicit-subject set must succeed");
    let got = extract_structured(
        &client
            .tools_call("host.state.get", json!({"key": "k", "end_user": "u5"}))
            .await
            .expect("explicit subject get"),
    );
    assert_eq!(got["value"], json!({"who": "u5"}), "{got:?}");
    assert_eq!(got["impersonated"], json!(true), "{got:?}");
}
