//! AC3 (P0) -- Given a first draft that fails `spec_test` and a second that
//! passes, When the call completes, Then the result has `attempts: 2`,
//! `test.ok: true`, `qualified_name`, `public_url` starting with the host's
//! `/x/` prefix, and the tool is callable by `host.tool_call`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::drafter::{Draft, FakeDrafter};
use mcphost::sandbox;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn failed_first_draft_then_passing_second_publishes_shares_and_is_callable() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let bad = Draft {
        name: Some("doubler".into()),
        spec: json!({"source": "def main(args):\n    return {\"doubled\": args[\"n\"] * undefined_name}\n"}),
        invocations: vec![json!({"n": 3})],
        ..Draft::default()
    };
    let good = Draft {
        name: Some("doubler".into()),
        spec: json!({"source": "def main(args):\n    return {\"doubled\": args[\"n\"] * 2}\n"}),
        invocations: vec![json!({"n": 3})],
        ..Draft::default()
    };
    let fake = Arc::new(FakeDrafter::new(vec![bad, good]));
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_drafter(python_kind_registry(&envs_dir.0), fake.clone()).await;
    let (ns, key) = signup(&server.base_url, "TFD AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let result = client
        .tools_call("host.tool_from_description", json!({"description": "double a number"}))
        .await
        .expect("second draft passes");
    let out = extract_structured(&result);
    assert_eq!(out["attempts"], json!(2), "{out}");
    assert_eq!(out["test"]["ok"], json!(true), "{out}");
    assert_eq!(out["qualified_name"], json!(format!("{ns}.doubler")), "{out}");
    let url = out["public_url"].as_str().expect("public_url");
    assert!(url.starts_with(&format!("{}/x/", server.base_url)), "public_url {url}");

    // The retry prompt carries the first failure's data as JSON.
    let prompts = fake.prompts();
    assert_eq!(prompts.len(), 2);
    assert!(prompts[1].contains("[previous_error]"), "retry prompt lacks the error data");

    let called = poll_until_ready(&client, &format!("{ns}.doubler"), json!({"n": 21}), Duration::from_secs(10))
        .await
        .unwrap_or_else(|e| panic!("host.tool_call failed: {} {}", e.code, e.message));
    assert_eq!(extract_structured(&called)["doubled"], json!(42));
}
