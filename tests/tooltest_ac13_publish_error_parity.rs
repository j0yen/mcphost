//! PRD-mcphost-tool-test AC13 — Given a `python` publish that fails
//! validation, When the error returns, Then it carries the same structured
//! detail fields as the `tool_test` (`host.spec_test`) response, so a
//! failed publish teaches as much as a failed test.

use crate::common;
use common::{McpClient, TestServer, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn missing_main_publish_error_carries_the_same_exception_class_as_spec_test() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "AC13 NoMain Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "def not_main(args):\n    return args\n",
        "args_schema": {"type": "object"},
    });

    let publish_err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "bad", "kind": "python", "spec": spec.clone()}),
        )
        .await
        .expect_err("publish of source without main() must be rejected");

    // Same structured fields `describe_test_failure` gives a failed test:
    // an `exception_class` naming what went wrong, not just a prose string.
    assert_eq!(
        publish_err.data["exception_class"],
        json!("NoMain"),
        "publish failure must carry exception_class: {:?}",
        publish_err.data
    );

    let test_err = client
        .tools_call(
            "host.spec_test",
            json!({"kind": "python", "spec": spec, "invocations": [{}]}),
        )
        .await
        .expect_err("spec_test of the same bad spec must fail validation identically");

    assert_eq!(
        publish_err.error_code, test_err.error_code,
        "publish and spec_test must fail with the same error_code"
    );
    assert_eq!(
        publish_err.data["exception_class"], test_err.data["exception_class"],
        "a failed publish must teach as much as a failed test: {:?} vs {:?}",
        publish_err.data, test_err.data
    );
}

#[tokio::test]
async fn syntax_error_publish_error_carries_the_same_exception_class_and_line_as_spec_test() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "AC13 Syntax Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "def main(args):\n    return (\n",
        "args_schema": {"type": "object"},
    });

    let publish_err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "bad_syntax", "kind": "python", "spec": spec.clone()}),
        )
        .await
        .expect_err("publish of unparseable source must be rejected");

    assert_eq!(
        publish_err.data["exception_class"],
        json!("SyntaxError"),
        "publish failure must name the exception class: {:?}",
        publish_err.data
    );
    assert_eq!(
        publish_err.data["line"],
        json!(2),
        "publish failure must name the offending line: {:?}",
        publish_err.data
    );

    let test_err = client
        .tools_call(
            "host.spec_test",
            json!({"kind": "python", "spec": spec, "invocations": [{}]}),
        )
        .await
        .expect_err("spec_test of the same unparseable spec must fail validation identically");

    assert_eq!(publish_err.error_code, test_err.error_code);
    assert_eq!(
        publish_err.data["exception_class"], test_err.data["exception_class"],
    );
    assert_eq!(publish_err.data["line"], test_err.data["line"]);
}
