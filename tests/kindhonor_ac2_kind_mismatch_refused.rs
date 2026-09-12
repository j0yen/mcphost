//! AC2 -- Given a spec with inline Python code published with `kind: http`,
//! When publish runs, Then it returns `kind_mismatch` with `inferred: python`
//! and a `reason` naming the inline code, and nothing is published.
//!
//! Also exercises the converse direction (an http-shaped spec requested as
//! `kind: python`) for the same requirement, since both directions of the
//! constraint are covered by the same `infer_kind_signal` logic.

use crate::common;
use common::{TempDataDir, TestServer, all_kinds_registry, signup};
use serde_json::json;

#[tokio::test]
async fn python_shaped_spec_requested_as_http_is_refused() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(all_kinds_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Kind Honor AC2a").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "cold_start_probe",
                "kind": "http",
                "spec": {"source": "def main(args):\n    return {\"ok\": True}\n"},
            }),
        )
        .await
        .expect_err("a python-shaped spec requested as http must be refused");

    assert_eq!(err.error_code.as_deref(), Some("kind_mismatch"));
    assert_eq!(err.data["requested"], json!("http"));
    assert_eq!(err.data["inferred"], json!("python"));
    let reason = err.data["reason"].as_str().expect("reason is a string");
    assert!(
        reason.contains("source"),
        "reason must name the inline-code field: {reason}"
    );

    // Nothing was published.
    let list = client
        .tools_call("host.tool_list", json!({}))
        .await
        .expect("host.tool_list");
    let tools = common::extract_structured(&list)["tools"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(tools.is_empty(), "refused publish must not create a tool row");
}

#[tokio::test]
async fn http_shaped_spec_requested_as_python_is_refused() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(all_kinds_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Kind Honor AC2b").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "weather",
                "kind": "python",
                "spec": {"method": "GET", "url": "https://api.example.com/weather"},
            }),
        )
        .await
        .expect_err("an http-shaped spec requested as python must be refused");

    assert_eq!(err.error_code.as_deref(), Some("kind_mismatch"));
    assert_eq!(err.data["requested"], json!("python"));
    assert_eq!(err.data["inferred"], json!("http"));
    let reason = err.data["reason"].as_str().expect("reason is a string");
    assert!(
        reason.contains("method") || reason.contains("url"),
        "reason must name the http-shaped fields: {reason}"
    );
}
