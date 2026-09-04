//! AC2 / AC5 (PRD-mcphost-publish-first-try requirement 2 / 3): every
//! rejection this test exercises carries `field`, `expected`, `docs`, and
//! (where a corrected value is known) `example` -- and resubmitting with
//! that `example` in place of the bad field is accepted. The last test adds
//! requirement 3's multi-error aggregation: two simultaneously-invalid
//! fields both come back in one rejection's `errors` array, each with its
//! own `field`/`expected`/`example`, and the corrected spec (both fields
//! fixed) is accepted on resubmission.

mod common;
use common::{McpClient, TestServer, http_kind_registry, signup};
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

/// Requirement 3 / AC2: "Given a publish with two invalid fields, When
/// rejected, Then the error lists both with field, expected, and example,
/// and the corrected example, resubmitted, is accepted." `method` and
/// `response` are independent fields on the `http` kind's spec -- both bad
/// at once must come back together, not one rejection per attempt.
#[tokio::test]
async fn two_invalid_fields_are_both_reported_and_the_corrected_spec_is_accepted() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "MultiFielder").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let bad_spec = json!({
        "method": "FETCH",
        "url": "https://127.0.0.1/items",
        "response": "xml",
        "args_schema": {"type": "object"},
    });
    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "my_tool", "kind": "http", "spec": bad_spec}),
        )
        .await
        .expect_err("two invalid fields must fail");

    let errors = err.data["errors"]
        .as_array()
        .expect("multi-field rejection must carry a data.errors array");
    assert_eq!(errors.len(), 2, "expected exactly 2 violations: {errors:?}");
    let fields: std::collections::BTreeSet<String> = errors
        .iter()
        .map(|e| {
            e["field"]
                .as_str()
                .expect("each error has a field")
                .to_string()
        })
        .collect();
    assert_eq!(
        fields,
        ["method", "response"]
            .into_iter()
            .map(String::from)
            .collect(),
        "{errors:?}"
    );
    for e in errors {
        assert!(e["expected"].as_str().is_some(), "{e:?}");
        assert!(
            e["example"].is_string() || e["example"].is_object(),
            "{e:?}"
        );
    }

    // The top-level field/expected/example (back-compat) name the first
    // violation, same as a single-field rejection would.
    assert!(err.data["field"].as_str().is_some());
    assert_eq!(err.data["docs"], json!("host.quickstart"));

    let method_example = errors.iter().find(|e| e["field"] == "method").unwrap()["example"].clone();
    let response_example =
        errors.iter().find(|e| e["field"] == "response").unwrap()["example"].clone();

    let mut fixed_spec = bad_spec.clone();
    fixed_spec["method"] = method_example;
    fixed_spec["response"] = response_example;
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "my_tool", "kind": "http", "spec": fixed_spec}),
        )
        .await
        .expect("the corrected spec (both fields fixed) must be accepted");
}
