//! PRD-mcphost-sandbox-bridge-discoverability
//! AC3 (P0) -- Given a python spec whose source contains `import host`,
//! When `host.spec_test` or `host.tool_publish` runs, Then the error class
//! is `unknown_import`, `data.hint` contains `import mcphost` and all
//! three module names, and nothing is published.

use crate::common;
use common::{TestServer, extract_structured, python_kind_registry, signup};
use serde_json::json;

const BAD_SOURCE: &str = "import host\ndef main(args):\n    return {}\n";

fn assert_unknown_import_hint(hint: &str) {
    assert!(hint.contains("import mcphost"), "hint must name the real import: {hint}");
    assert!(hint.contains("mcphost.state"), "hint must name mcphost.state: {hint}");
    assert!(hint.contains("mcphost.table"), "hint must name mcphost.table: {hint}");
    assert!(hint.contains("mcphost.docs"), "hint must name mcphost.docs: {hint}");
}

#[tokio::test]
async fn tool_publish_rejects_import_host_as_unknown_import_and_publishes_nothing() {
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Bridgedisc AC3 Publish Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "bad_import_tool", "kind": "python", "spec": {"source": BAD_SOURCE}}),
        )
        .await
        .expect_err("a source that imports host must be refused");
    assert_eq!(err.data["error_code"], json!("unknown_import"));
    assert_unknown_import_hint(err.data["hint"].as_str().expect("data.hint is a string"));

    let listed = extract_structured(
        &client.tools_call("host.tool_list", json!({})).await.expect("host.tool_list"),
    );
    let names: Vec<&str> = listed["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(
        !names.contains(&"bad_import_tool"),
        "a rejected publish must leave nothing published: {names:?}"
    );
}

#[tokio::test]
async fn spec_test_rejects_import_host_as_unknown_import() {
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Bridgedisc AC3 SpecTest Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(
            "host.spec_test",
            json!({"kind": "python", "spec": {"source": BAD_SOURCE}, "invocations": [{}]}),
        )
        .await
        .expect_err("a source that imports host must be refused");
    assert_eq!(err.data["error_code"], json!("unknown_import"));
    assert_unknown_import_hint(err.data["hint"].as_str().expect("data.hint is a string"));
}

#[tokio::test]
async fn from_host_import_and_mcphost_sdk_are_also_rejected() {
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Bridgedisc AC3 Variants Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    for (label, source) in [
        ("from_host_import", "from host import state\ndef main(args):\n    return {}\n"),
        ("import_mcphost_sdk", "import mcphost_sdk\ndef main(args):\n    return {}\n"),
        (
            "unregistered_mcphost_attr",
            "import mcphost\ndef main(args):\n    return mcphost.not_a_real_module.get()\n",
        ),
    ] {
        let err = client
            .tools_call(
                "host.tool_publish",
                json!({"name": format!("bad_{label}"), "kind": "python", "spec": {"source": source}}),
            )
            .await
            .expect_err(&format!("{label} must be refused, not published"));
        assert_eq!(err.data["error_code"], json!("unknown_import"), "{label}: {err:?}");
    }
}
