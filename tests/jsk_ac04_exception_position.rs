//! mcphost-js-kind AC4 (P0) — Given a tool that throws on line 3, When called, Then
//! `error_code: tool_exception`, the message ends `at tool.js:3`,
//! `data.source_line` is that line, `data.families` is `["tool_code"]`.

use crate::common;
use common::{TestServer, js_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn a_throw_on_line_three_is_tool_exception_with_its_source_line() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(js_kind_registry(&dir.0)).await;
    let (ns, key) = signup(&server.base_url, "JSK AC4 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "function main(args) {\n  const x = 1;\n  throw new TypeError(\"boom \" + x);\n}\n",
        "args_schema": {"type": "object"},
    });
    client
        .tools_call("host.tool_publish", json!({"name": "thrower", "kind": "js", "spec": spec}))
        .await
        .expect("publish ok");

    let err = client
        .tools_call(&format!("{ns}.thrower"), json!({}))
        .await
        .expect_err("a throwing tool must fail");
    assert_eq!(err.error_code.as_deref(), Some("tool_exception"), "{err:?}");
    assert!(err.message.ends_with("at tool.js:3"), "message: {}", err.message);
    assert!(err.message.starts_with("TypeError: boom 1"), "message: {}", err.message);
    assert_eq!(err.data["source_line"], json!("throw new TypeError(\"boom \" + x);"), "{err:?}");
    assert_eq!(err.data["line"], json!(3), "{err:?}");
    assert_eq!(err.data["families"], json!(["tool_code"]), "{err:?}");
}
