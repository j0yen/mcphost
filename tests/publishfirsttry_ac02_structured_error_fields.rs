//! Partial AC2 / AC5 (PRD-mcphost-publish-first-try requirement 2): every
//! rejection this test exercises carries `field`, `expected`, `docs`, and
//! (where a corrected value is known) `example` -- and resubmitting with
//! that `example` in place of the bad field is accepted.
//!
//! NOT covered here (see the PRD frontmatter's `deferred_acs`): reporting
//! *multiple* simultaneously-invalid fields in one rejection (requirement
//! 3's multi-error aggregation) -- every case below has exactly one bad
//! field, which is the part this tick actually implemented.

mod common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn invalid_tool_name_carries_field_expected_example_and_the_example_is_accepted() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Fielder").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let good_spec = json!({"schema": {"type": "object"}});
    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "Not-Valid!", "kind": "echo", "spec": good_spec}),
        )
        .await
        .expect_err("invalid name must fail");
    assert_eq!(err.error_code.as_deref(), Some("invalid_tool_name"));
    assert_eq!(err.data["field"], json!("name"));
    assert!(err.data["expected"].as_str().is_some(), "{:?}", err.data);
    assert_eq!(err.data["docs"], json!("host.quickstart"));
    let example = err.data["example"]
        .as_str()
        .expect("invalid_tool_name carries a corrected example")
        .to_string();

    // The corrected example, resubmitted, is accepted.
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": example, "kind": "echo", "spec": good_spec}),
        )
        .await
        .expect("the corrected example name must be accepted");
}

#[tokio::test]
async fn echo_missing_schema_carries_field_and_example_and_the_example_is_accepted() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Fielder").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "my_tool", "kind": "echo", "spec": {}}),
        )
        .await
        .expect_err("missing spec.schema must fail");
    assert_eq!(err.error_code.as_deref(), Some("invalid_spec"));
    assert_eq!(err.data["field"], json!("spec.schema"));
    assert!(err.data["expected"].as_str().is_some(), "{:?}", err.data);
    let example = err.data["example"].clone();
    assert!(
        example.is_object(),
        "spec.schema example must be a schema object: {example}"
    );

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "my_tool", "kind": "echo", "spec": {"schema": example}}),
        )
        .await
        .expect("the corrected schema example must be accepted");
}

#[tokio::test]
async fn unknown_kind_carries_field_and_docs() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Fielder").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "my_tool", "kind": "not_a_kind", "spec": {}}),
        )
        .await
        .expect_err("unknown kind must fail");
    assert_eq!(err.error_code.as_deref(), Some("unknown_kind"));
    assert_eq!(err.data["field"], json!("kind"));
    assert_eq!(err.data["docs"], json!("host.quickstart"));
}
