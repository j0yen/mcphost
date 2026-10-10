//! PRD-mcphost-unknown-import-domain-hint
//! AC4 (P0) -- Given a tool whose code does
//! `importlib.import_module("mcphost.dev")` at run time, When called, Then
//! the run-time error has the identical code, message and `data` as the
//! publish-time one.

use crate::common;
use common::{McpClient, TestServer, poll_until_ready, python_kind_registry_with_domain, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

const STATIC_SOURCE: &str = "from mcphost.dev import table\ndef main(a):\n    return {}\n";
// The static scan flags any literal `mcphost.<unknown>` on a line, so the
// dynamic import builds the name from parts: it still calls
// `importlib.import_module("mcphost.dev")` at run time, but only then.
const DYNAMIC_SOURCE: &str = "import importlib\n\n\ndef main(args):\n    name = \"mcphost\" + \".\" + \"dev\"\n    importlib.import_module(name)\n    return {\"ok\": True}\n";

#[tokio::test]
async fn runtime_domain_import_error_equals_the_publish_time_one() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server =
        TestServer::start_with_kinds(python_kind_registry_with_domain(&envs_dir.0, "mcphost.dev")).await;
    let (ns, key) = signup(&server.base_url, "Domhint AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let publish_err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "static_domain", "kind": "python", "spec": {"source": STATIC_SOURCE}}),
        )
        .await
        .expect_err("the static scan refuses the domain import");

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "dynamic_domain", "kind": "python", "spec": {"source": DYNAMIC_SOURCE}}),
        )
        .await
        .expect("a dynamic import escapes the static scan and publishes");
    let qualified = format!("{ns}.dynamic_domain");
    let run_err = poll_until_ready(&client, &qualified, json!({}), Duration::from_secs(10))
        .await
        .expect_err("the dynamic domain import must fail the call");

    assert_eq!(run_err.data["error_code"], json!("unknown_import"), "{run_err:?}");
    assert_eq!(run_err.data["error_code"], publish_err.data["error_code"]);
    assert_eq!(run_err.message, publish_err.message);
    for field in ["hint", "module", "own_domain"] {
        assert_eq!(run_err.data[field], publish_err.data[field], "data.{field}");
    }
    assert_eq!(run_err.data["own_domain"], json!("mcphost.dev"));
    assert!(run_err.data.get("traceback").is_none(), "{:?}", run_err.data);
}
