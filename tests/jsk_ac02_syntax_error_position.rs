//! mcphost-js-kind AC2 (P0) — Given `spec.source` with a syntax error on line 2, When
//! published, Then `InvalidSpec` carries `line: 2`, `column`, and
//! `source_line` with that line's text.

use crate::common;
use common::{TestServer, js_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn a_syntax_error_on_line_two_is_rejected_with_line_column_and_source_line() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(js_kind_registry(&dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "JSK AC2 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({"source": "function main(args) {\n  let = = 2;\n  return {};\n}\n"});
    let err = client
        .tools_call("host.tool_publish", json!({"name": "bad_syntax", "kind": "js", "spec": spec}))
        .await
        .expect_err("publish of unparseable js source must be rejected");

    assert_eq!(err.error_code.as_deref(), Some("invalid_spec"), "{err:?}");
    assert_eq!(err.data["line"], json!(2), "data.line: {err:?}");
    assert!(err.data["column"].as_i64().is_some_and(|c| c >= 1), "data.column: {err:?}");
    assert_eq!(err.data["source_line"], json!("let = = 2;"), "data.source_line: {err:?}");
    assert_eq!(err.data["field"], json!("source"), "{err:?}");
}

#[tokio::test]
async fn a_source_without_main_is_rejected_naming_source() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(js_kind_registry(&dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "JSK AC2b Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({"source": "function not_main(args) { return args; }\n"});
    let err = client
        .tools_call("host.tool_publish", json!({"name": "no_main", "kind": "js", "spec": spec}))
        .await
        .expect_err("publish of source without main must be rejected");
    assert!(err.message.contains("source"), "error must name 'source': {}", err.message);
}
