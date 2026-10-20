//! PRD-mcphost-host-args-validated-by-default
//! AC2 (P0) -- Given a `TestServer` over the production `build_router`, When
//! `host.example.list` gets an out-of-set `use_case`, `host.agent.contacts` a
//! non-string `status` and `admin.tenants` an out-of-set `synthetic`, Then each
//! is refused `args_invalid` at its own path with the schema's own `expected`
//! value, and `handler_runs` proves none of the handler bodies ran.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, signup};
use mcphost::examples::EXAMPLE_USE_CASES;
use mcphost::host_args::handler_runs;
use serde_json::json;

#[tokio::test]
async fn vbd_ac02_three_formerly_unvalidated_verbs_refuse_before_their_handler() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "VBD AC2").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    // `admin.tenants` counts under its `prefix`, so this test owns its key.
    let admin_ns = format!("vbd-ac02-{ns}");

    let list_before = handler_runs(&ns, "host.example.list");
    let err = client
        .tools_call("host.example.list", json!({"use_case": "not-a-use-case"}))
        .await
        .expect_err("an unknown use_case must be refused");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{err:?}");
    assert_eq!(err.data["path"], json!("/use_case"), "{:?}", err.data);
    assert_eq!(err.data["constraint"], json!("enum"), "{:?}", err.data);
    assert_eq!(err.data["expected"], json!(EXAMPLE_USE_CASES), "{:?}", err.data);
    assert_eq!(handler_runs(&ns, "host.example.list"), list_before, "host.example.list ran");

    let contacts_before = handler_runs(&ns, "host.agent.contacts");
    let err = client
        .tools_call("host.agent.contacts", json!({"status": 5}))
        .await
        .expect_err("a numeric status must be refused");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{err:?}");
    assert_eq!(err.data["path"], json!("/status"), "{:?}", err.data);
    assert_eq!(err.data["constraint"], json!("type"), "{:?}", err.data);
    assert_eq!(err.data["expected"], json!("string"), "{:?}", err.data);
    assert_eq!(handler_runs(&ns, "host.agent.contacts"), contacts_before, "host.agent.contacts ran");

    let tenants_before = handler_runs(&admin_ns, "admin.tenants");
    let err = admin
        .tools_call("admin.tenants", json!({"prefix": admin_ns, "synthetic": "maybe"}))
        .await
        .expect_err("an unknown synthetic must be refused");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{err:?}");
    assert_eq!(err.data["path"], json!("/synthetic"), "{:?}", err.data);
    assert_eq!(err.data["constraint"], json!("enum"), "{:?}", err.data);
    assert_eq!(err.data["expected"], json!(["true", "false", "all"]), "{:?}", err.data);
    assert_eq!(handler_runs(&admin_ns, "admin.tenants"), tenants_before, "admin.tenants ran");

    // The same verbs still run for a valid call.
    client.tools_call("host.example.list", json!({})).await.expect("valid list");
    assert_eq!(handler_runs(&ns, "host.example.list"), list_before + 1);
    admin.tools_call("admin.tenants", json!({"prefix": admin_ns})).await.expect("valid tenants");
    assert_eq!(handler_runs(&admin_ns, "admin.tenants"), tenants_before + 1);
}
