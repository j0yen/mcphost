//! PRD-mcphost-spec-unknown-field-rejection
//! AC1 — Given a python spec with an extra key `write_table`, When
//! `host.tool_publish` or `host.spec_test` runs, Then the error class is
//! `unknown_spec_field`, `data.field == "write_table"`, `data.known` lists
//! the python fields, and no tool is published.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

fn bad_spec() -> serde_json::Value {
    json!({"source": "def main(args):\n    return args\n", "write_table": "runs"})
}

#[tokio::test]
async fn tool_publish_rejects_write_table_and_writes_nothing() {
    let server = TestServer::start_with_kinds(common::python_kind_registry(
        server_data_dir().as_path(),
    ))
    .await;
    let (tenant_ns, key) = signup(&server.base_url, "Unkfield AC1").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "hello", "kind": "python", "spec": bad_spec()}),
        )
        .await
        .expect_err("an unknown spec field must fail the publish");

    assert_eq!(err.error_code.as_deref(), Some("unknown_spec_field"));
    assert_eq!(err.data.get("field"), Some(&json!("write_table")));
    let known = err
        .data
        .get("known")
        .and_then(|v| v.as_array())
        .expect("data.known is an array")
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert!(known.contains(&"source".to_string()), "known: {known:?}");
    assert!(
        known.contains(&"requirements".to_string()),
        "known: {known:?}"
    );

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(tenant_ns)
        .await
        .unwrap()
        .expect("tenant exists");
    let tools = server.state.db.list_tools(tenant.id).await.unwrap();
    assert!(
        tools.is_empty(),
        "a rejected publish must not have written a tool row: {tools:?}"
    );
}

#[tokio::test]
async fn spec_test_rejects_write_table() {
    let server = TestServer::start_with_kinds(common::python_kind_registry(
        server_data_dir().as_path(),
    ))
    .await;
    let (_ns, key) = signup(&server.base_url, "Unkfield AC1 spec_test").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(
            "host.spec_test",
            json!({"kind": "python", "spec": bad_spec(), "invocations": [{}]}),
        )
        .await
        .expect_err("host.spec_test must also reject an unknown spec field");

    assert_eq!(err.error_code.as_deref(), Some("unknown_spec_field"));
    assert_eq!(err.data.get("field"), Some(&json!("write_table")));
}

/// A fresh temp dir per test run, matching the convention
/// `common::python_kind_registry` callers elsewhere in this suite use
/// (`PythonKind::new` roots its env cache under it).
fn server_data_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-unkfield-ac01-{}-{}",
        std::process::id(),
        mcphost::state::now_unix()
    ));
    std::fs::create_dir_all(&dir).expect("create temp data dir");
    dir
}
